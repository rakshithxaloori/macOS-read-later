use crate::whatsapp::{
    WhatsAppCommand, WhatsAppConfig, WhatsAppEvent, WhatsAppHandle, WhatsAppView,
};
use crate::{
    backend::{Backend, Command, Event, TransferId, TransferKind, TransferResult},
    domain::*,
    native_pdf::PdfWindow,
    paths::AppDirs,
    rich_content::RichReader,
    settings::Settings,
    ui::{self, Action, View},
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::mpsc::TrySendError;

#[derive(Clone)]
enum Dialog {
    Save,
    Feed,
    Delete(Vec<ArticleId>),
    DeleteMatching { query: Query, expected: u32 },
    Details { title: String, message: String },
}
/// How soon a reload that found the database queue full is tried again.
const RELOAD_RETRY: std::time::Duration = std::time::Duration::from_millis(200);

enum ReloadStep {
    List,
    Summary,
    Selected,
}

/// Reloads owed to the UI. Change signals pile up while a hidden window draws
/// no frames, and one reload covers any number of them. A step stays owed until
/// the database queue accepts it, so a full queue cannot leave the list stale.
#[derive(Default)]
struct PendingReload {
    list: bool,
    summary: bool,
    selected: bool,
}
impl PendingReload {
    fn pending(&self) -> bool {
        self.list || self.summary || self.selected
    }
    fn flush(&mut self, mut queue: impl FnMut(ReloadStep) -> bool) {
        if self.list && queue(ReloadStep::List) {
            self.list = false;
            self.summary = true;
        }
        if self.summary && queue(ReloadStep::Summary) {
            self.summary = false;
        }
        if self.selected && queue(ReloadStep::Selected) {
            self.selected = false;
        }
    }
}

pub struct App {
    repaint: egui::Context,
    reload: PendingReload,
    dirs: AppDirs,
    settings: Settings,
    backend: Backend,
    view: View,
    rows: Vec<Article>,
    publications: Vec<(String, u32)>,
    counts: Counts,
    feeds: Vec<Feed>,
    magazines: Vec<Magazine>,
    selected: Option<ArticleBody>,
    reader: RichReader,
    generation: u64,
    body_generation: u64,
    loading: bool,
    body_loading: bool,
    body_error: Option<String>,
    fetched_at: Option<String>,
    has_more: bool,
    matching_count: u32,
    logos: std::collections::HashMap<String, egui::TextureHandle>,
    dialog: Option<Dialog>,
    dialog_input: ui::dialog::Input,
    selection: Vec<ArticleId>,
    selection_metadata: std::collections::HashMap<ArticleId, Article>,
    selecting: bool,
    selecting_delete: bool,
    selection_return: Option<View>,
    transfer_generation: u64,
    transfer_activity: Option<(TransferId, TransferKind)>,
    error: String,
    notice: String,
    quit: bool,
    pdfs: Vec<PdfWindow>,
    runtime: tokio::runtime::Runtime,
    whatsapp: Option<WhatsAppHandle>,
    whatsapp_events: tokio::sync::mpsc::Receiver<WhatsAppEvent>,
    whatsapp_view: WhatsAppView,
    phone: String,
    offline: bool,
    qr: Option<egui::TextureHandle>,
    qr_payload: Option<String>,
    imports: Vec<crate::imports::Progress>,
    native_reader: Option<crate::rich_content::native::NativeRichView>,
    native_dirty: bool,
    controls_keyboard_focus: Option<egui::Id>,
    instance: crate::instance::Instance,
    intake: Option<crate::intake::Handle>,
    intake_events: std::sync::mpsc::Receiver<crate::intake::Event>,
    intake_enabled: bool,
    magazine_state: ui::magazine::State,
    production: Option<crate::magazine::Worker>,
    production_events: std::sync::mpsc::Receiver<()>,
}
impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        dirs: AppDirs,
        offline: bool,
        instance: crate::instance::Instance,
    ) -> Self {
        configure(&cc.egui_ctx);
        let settings = Settings::load(&dirs.settings());
        crate::theme::apply(&cc.egui_ctx, settings.appearance);
        let backend = Backend::spawn(dirs.database(), cc.egui_ctx.clone(), offline);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Tokio background runtime");
        let (wa_tx, whatsapp_events) = tokio::sync::mpsc::channel(64);
        let ctx = cc.egui_ctx.clone();
        let wake = std::sync::Arc::new(move || ctx.request_repaint());
        instance.set_waker(wake.clone());
        crate::native_menu::install(wake.clone());
        let (intake_tx, intake_events) = std::sync::mpsc::sync_channel(64);
        let mut startup_error = String::new();
        let intake = match crate::intake::spawn(
            crate::intake::Config {
                library_path: dirs.database(),
                private_directory: dirs.data.join("intake"),
            },
            intake_tx,
            wake.clone(),
        ) {
            Ok(handle) => {
                if !offline {
                    let _ = handle.command(crate::intake::Command::Enable(true));
                }
                Some(handle)
            }
            Err(error) => {
                startup_error = format!("Background intake could not start: {error}");
                None
            }
        };
        let (production_tx, production_events) = std::sync::mpsc::sync_channel(8);
        let production_ctx = cc.egui_ctx.clone();
        let production = Some(crate::magazine::spawn_worker(
            dirs.database(),
            dirs.data.join("production"),
            std::sync::Arc::new(move || {
                let _ = production_tx.try_send(());
                production_ctx.request_repaint();
            }),
        ));
        let whatsapp = {
            let _guard = runtime.enter();
            Some(crate::whatsapp::spawn(
                WhatsAppConfig {
                    library_path: dirs.database(),
                    private_directory: dirs.data.join("whatsapp"),
                    offline,
                },
                wa_tx,
                wake,
            ))
        };
        let mut app = Self {
            dirs,
            settings,
            backend,
            view: View::Inbox,
            rows: vec![],
            publications: vec![],
            counts: Counts::default(),
            feeds: vec![],
            magazines: vec![],
            selected: None,
            reader: RichReader::default(),
            generation: 0,
            body_generation: 0,
            loading: false,
            body_loading: false,
            body_error: None,
            fetched_at: None,
            has_more: false,
            matching_count: 0,
            repaint: cc.egui_ctx.clone(),
            reload: PendingReload::default(),
            logos: std::collections::HashMap::new(),
            dialog: None,
            dialog_input: ui::dialog::Input::default(),
            selection: vec![],
            selection_metadata: std::collections::HashMap::new(),
            selecting: false,
            selecting_delete: false,
            selection_return: None,
            transfer_generation: 0,
            transfer_activity: None,
            error: startup_error,
            notice: String::new(),
            quit: false,
            pdfs: vec![],
            runtime,
            whatsapp,
            whatsapp_events,
            whatsapp_view: WhatsAppView::default(),
            phone: String::new(),
            offline,
            qr: None,
            qr_payload: None,
            imports: vec![],
            native_reader: None,
            native_dirty: true,
            controls_keyboard_focus: None,
            instance,
            intake,
            intake_events,
            intake_enabled: !offline,
            magazine_state: Default::default(),
            production,
            production_events,
        };
        app.refresh();
        app
    }
    fn query(&self) -> &Query {
        if self.view == View::Archive {
            &self.settings.archive
        } else {
            &self.settings.inbox
        }
    }
    fn send(&mut self, c: Command) {
        if let Command::Refetch(id) = &c
            && self.selected.as_ref().is_some_and(|b| b.article.id == *id)
        {
            self.body_error = None;
            self.body_loading = true;
            self.fetched_at = None;
            self.body_generation += 1;
            let _ = self.backend.send(Command::CancelBody);
            if let Err(error) = self.backend.send(Command::RefreshBody {
                generation: self.body_generation,
                id: id.clone(),
            }) {
                self.body_loading = false;
                self.body_error = Some(error);
            }
            return;
        }
        let is_body_request = matches!(&c, Command::Body { .. } | Command::RefreshBody { .. });
        if !matches!(
            c,
            Command::List { .. }
                | Command::Reconcile { .. }
                | Command::Body { .. }
                | Command::CancelBody
                | Command::RefreshBody { .. }
                | Command::RefreshSelected { .. }
                | Command::FetchedMetadata(_)
                | Command::Summary
                | Command::Pdf(_)
                | Command::Imports
                | Command::Export { .. }
        ) && let Some(intake) = &self.intake
        {
            let _ = intake.command(crate::intake::Command::Wake);
        }
        if let Err(e) = self.backend.send(c) {
            if is_body_request {
                self.body_loading = false;
                self.body_error = Some(e.clone());
            }
            self.error = e;
        }
    }
    fn refresh(&mut self) {
        self.reload.list = true;
        self.flush_reload();
    }
    fn flush_reload(&mut self) {
        let mut reload = std::mem::take(&mut self.reload);
        reload.flush(|step| self.queue_reload(step));
        if reload.pending() {
            self.repaint.request_repaint_after(RELOAD_RETRY);
        }
        self.reload = reload;
    }
    fn queue_reload(&mut self, step: ReloadStep) -> bool {
        let command = match step {
            ReloadStep::List => {
                self.generation += 1;
                self.loading = true;
                let loaded = if self.view == View::Archive {
                    self.settings.archive_loaded
                } else {
                    self.settings.inbox_loaded
                }
                .max(60);
                Command::Reconcile {
                    generation: self.generation,
                    query: self.query().clone(),
                    loaded: loaded as usize,
                }
            }
            ReloadStep::Summary => Command::Summary,
            ReloadStep::Selected => {
                let Some(id) = self.selected.as_ref().map(|b| b.article.id.clone()) else {
                    return true;
                };
                Command::RefreshSelected {
                    generation: self.body_generation,
                    id,
                }
            }
        };
        match self.backend.commands.try_send(command) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => {
                self.error = "The library database stopped responding.".into();
                true
            }
        }
    }
    fn open(&mut self, id: ArticleId) {
        if self
            .selected
            .as_ref()
            .is_some_and(|b| b.article.id == id && (self.body_loading || !b.html.is_empty()))
        {
            return;
        }
        self.body_generation += 1;
        self.body_loading = true;
        self.body_error = None;
        self.fetched_at = None;
        if let Some(article) = self.rows.iter().find(|a| a.id == id).cloned().or_else(|| {
            self.selected
                .as_ref()
                .filter(|b| b.article.id == id)
                .map(|b| b.article.clone())
        }) {
            self.selected = Some(ArticleBody {
                article,
                html: String::new(),
                media: vec![],
            });
            self.reader.set_html("");
            self.native_dirty = true;
        }
        self.send(Command::Body {
            generation: self.body_generation,
            id,
        });
    }
    fn events(&mut self, ctx: &egui::Context) {
        let mut produced = false;
        while self.production_events.try_recv().is_ok() {
            produced = true;
        }
        if produced && self.magazine_state.editing.is_none() {
            if let Some(id) = self
                .magazine_state
                .edition
                .as_ref()
                .map(|e| e.issue.id.clone())
            {
                self.send(Command::Magazine(crate::magazine::Command::Open(id)));
            }
            self.reload.summary = true;
        }
        while let Ok(event) = self.intake_events.try_recv() {
            match event {
                crate::intake::Event::LibraryChanged => self.reload.list = true,
                crate::intake::Event::State { enabled, error } => {
                    self.intake_enabled = enabled;
                    self.reload.summary = true;
                    if let Some(error) = error {
                        self.error = error;
                    }
                }
            }
        }
        while let Ok(event) = self.whatsapp_events.try_recv() {
            match event {
                WhatsAppEvent::State(state) => {
                    self.whatsapp_view = state;
                    self.update_qr(ctx);
                }
                WhatsAppEvent::LibraryChanged => self.reload.list = true,
            }
        }
        while let Ok(e) = self.backend.events.try_recv() {
            match e {
                Event::Logos { generation, logos } => {
                    if generation == self.generation {
                        for (url, bytes) in logos {
                            if !self.logos.contains_key(&url)
                                && let Ok(image) = image::load_from_memory(&bytes)
                            {
                                let image = image.thumbnail(72, 72).to_rgba8();
                                self.logos.insert(
                                    url.clone(),
                                    ctx.load_texture(
                                        url,
                                        egui::ColorImage::from_rgba_unmultiplied(
                                            [image.width() as usize, image.height() as usize],
                                            image.as_raw(),
                                        ),
                                        egui::TextureOptions::LINEAR,
                                    ),
                                );
                            }
                        }
                    }
                }
                Event::List {
                    generation,
                    offset,
                    articles,
                    publications,
                    total,
                    has_more,
                    logos,
                } => {
                    if generation == self.generation {
                        self.has_more = has_more;
                        self.matching_count = total;
                        for (url, bytes) in logos {
                            if !self.logos.contains_key(&url)
                                && let Ok(image) = image::load_from_memory(&bytes)
                            {
                                let image = image.thumbnail(72, 72).to_rgba8();
                                self.logos.insert(
                                    url.clone(),
                                    ctx.load_texture(
                                        url,
                                        egui::ColorImage::from_rgba_unmultiplied(
                                            [image.width() as usize, image.height() as usize],
                                            image.as_raw(),
                                        ),
                                        egui::TextureOptions::LINEAR,
                                    ),
                                );
                            }
                        }
                        self.publications = publications;
                        if offset == 0 {
                            self.rows = articles;
                        } else {
                            for a in articles {
                                if !self.rows.iter().any(|r| r.id == a.id) {
                                    self.rows.push(a);
                                }
                            }
                        }
                        if self.view == View::Archive {
                            self.settings.archive_loaded = self.rows.len() as u32;
                        } else {
                            self.settings.inbox_loaded = self.rows.len() as u32;
                        }
                        self.loading = false;
                    }
                }
                Event::Body {
                    generation,
                    mut body,
                    fetched_at,
                } => {
                    if generation == self.body_generation {
                        if let Some(selected) = self
                            .selected
                            .as_ref()
                            .filter(|s| s.article.id == body.article.id)
                        {
                            body.article.read_at = selected.article.read_at.clone();
                            body.article.archived_at = selected.article.archived_at.clone();
                        }
                        self.reader.set_html(&body.html);
                        for (id, bytes) in &body.media {
                            if let Err(e) =
                                self.reader
                                    .cache_image(ctx, &format!("/api/media/{id}"), bytes)
                            {
                                self.error = e;
                            }
                        }
                        self.send(Command::FetchedMetadata(Box::new(body.article.clone())));
                        self.selected = Some(*body);
                        self.fetched_at = fetched_at;
                        self.body_error = None;
                        self.native_dirty = true;
                        self.body_loading = false;
                    }
                }
                Event::BodyError {
                    generation,
                    id,
                    message,
                } => {
                    if generation == self.body_generation
                        && self.selected.as_ref().is_some_and(|b| b.article.id == id)
                    {
                        self.body_loading = false;
                        self.body_error = Some(message);
                        self.native_dirty = true;
                    }
                }
                Event::SelectedMetadata {
                    generation,
                    article,
                } => {
                    if generation == self.body_generation {
                        if let Some(article) = article {
                            if let Some(body) = self.selected.as_mut() {
                                body.article = article;
                                self.native_dirty = true;
                            }
                        } else {
                            self.selected = None;
                            self.body_generation += 1;
                            self.send(Command::CancelBody);
                        }
                    }
                }
                Event::MagazineProgress(message) => {
                    self.magazine_state.progress = Some(message);
                }
                Event::Summary {
                    counts,
                    feeds,
                    magazines,
                } => {
                    self.counts = counts;
                    self.feeds = feeds;
                    self.magazines = magazines;
                }
                Event::Magazine(event) => {
                    if matches!(&event,crate::magazine::Event::Edition(e)if e.production.as_ref().is_some_and(|p|p.state==crate::magazine::ProductionState::Queued))
                        && let Some(worker) = &self.production
                    {
                        let _ = worker
                            .commands
                            .try_send(crate::magazine::WorkerCommand::Wake);
                    }
                    let reload = matches!(&event, crate::magazine::Event::Covers(_));
                    self.magazine_state.receive(event);
                    self.reload.summary = true;
                    if reload
                        && let Some(id) = self
                            .magazine_state
                            .edition
                            .as_ref()
                            .map(|e| e.issue.id.clone())
                    {
                        self.send(Command::Magazine(crate::magazine::Command::Open(id)));
                    }
                }
                Event::Imports(progress) => self.imports = progress,
                Event::TransferFinished { request_id, result } => {
                    if self
                        .transfer_activity
                        .is_some_and(|(active, _)| active == request_id)
                    {
                        self.transfer_activity = None;
                        match result {
                            Ok(TransferResult::ImportPreview(progress)) => {
                                self.imports = progress;
                                self.notice = "Import preview ready.".into();
                            }
                            Ok(TransferResult::Exported) => {
                                self.notice = "Library exported.".into()
                            }
                            Err(error) => self.error = error,
                        }
                    }
                }
                Event::Changed => {
                    if let Some(intake) = &self.intake {
                        let _ = intake.command(crate::intake::Command::Wake);
                    }
                    self.reload.list = true;
                    self.reload.selected = true;
                }
                Event::Pdf { id, title, bytes } => {
                    match PdfWindow::open(&bytes, &format!("{title} · Read Later")) {
                        Ok(p) => {
                            p.set_appearance(self.settings.appearance);
                            if self
                                .magazine_state
                                .edition
                                .as_ref()
                                .is_some_and(|edition| edition.issue.id == id)
                            {
                                self.magazine_state.opened_pdf_sha256 =
                                    Some(p.original_sha256.clone());
                            }
                            self.pdfs.push(p);
                        }
                        Err(e) => {
                            if self
                                .magazine_state
                                .edition
                                .as_ref()
                                .is_some_and(|edition| edition.issue.id == id)
                            {
                                self.magazine_state.opened_pdf_sha256 = None;
                            }
                            self.error = e;
                        }
                    }
                }
                Event::Notice(s) => self.notice = s,
                Event::Error(s) => {
                    self.error = s;
                    self.loading = false;
                    self.body_loading = false;
                    self.magazine_state.loading = false;
                    self.magazine_state.progress = None;
                }
            }
        }
        self.flush_reload();
    }
    fn begin_transfer(
        &mut self,
        kind: TransferKind,
        path: std::path::PathBuf,
        ctx: &egui::Context,
    ) {
        if self.transfer_activity.is_some() {
            return;
        }
        self.transfer_generation += 1;
        let request_id = TransferId(self.transfer_generation);
        let command = match kind {
            TransferKind::Import => Command::StageImport { request_id, path },
            TransferKind::Export => Command::Export { request_id, path },
        };
        match self.backend.send(command) {
            Ok(()) => self.transfer_activity = Some((request_id, kind)),
            Err(error) => self.error = error,
        }
        ctx.request_repaint();
    }
    fn clear_selection(&mut self) {
        self.selection.clear();
        self.selection_metadata.clear();
    }
    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Navigate(v) => {
                self.rows.clear();
                self.view = v;
                self.clear_selection();
                self.selecting = false;
                self.selection_return = None;
                self.selected = None;
                self.body_generation += 1;
                self.send(Command::CancelBody);
                if matches!(v, View::Inbox | View::Archive) {
                    self.refresh();
                } else {
                    self.send(Command::Summary);
                    if v == View::ImportExport {
                        self.send(Command::Imports);
                    }
                }
            }
            Action::Open(id) => self.open(id),
            Action::CloseReader => {
                self.body_generation += 1;
                self.selected = None;
                self.body_loading = false;
                self.body_error = None;
                self.fetched_at = None;
                self.send(Command::CancelBody);
            }
            Action::Query(q) => {
                if q.scope == Scope::Archive {
                    self.settings.archive = q;
                    self.settings.archive_scroll = 0.;
                    self.settings.archive_loaded = 60;
                } else {
                    self.settings.inbox = q;
                    self.settings.inbox_scroll = 0.;
                    self.settings.inbox_loaded = 60;
                }
                self.rows.clear();
                self.refresh();
            }
            Action::LoadMore => {
                if !self.loading {
                    self.loading = true;
                    self.send(Command::List {
                        generation: self.generation,
                        query: self.query().clone(),
                        offset: self.rows.len(),
                    });
                }
            }
            Action::Command(c) => self.send(c),
            Action::ChooseImport => {
                if self.transfer_activity.is_some() {
                    return;
                }
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "Reading data",
                        &["csv", "zip", "json", "opml", "html", "htm", "xml"],
                    )
                    .pick_file()
                {
                    self.begin_transfer(TransferKind::Import, path, ctx);
                }
            }
            Action::ChooseExport => {
                if self.transfer_activity.is_some() {
                    return;
                }
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("macOS-read-later.zip")
                    .add_filter("ZIP", &["zip"])
                    .save_file()
                {
                    self.begin_transfer(TransferKind::Export, path, ctx);
                }
            }
            Action::SaveDialog => {
                if let Some(view) = &self.native_reader {
                    view.focus_controls();
                }
                self.dialog_input = ui::dialog::Input {
                    focus: true,
                    ..Default::default()
                };
                self.dialog = Some(Dialog::Save);
            }
            Action::FeedDialog => {
                if let Some(view) = &self.native_reader {
                    view.focus_controls();
                }
                self.dialog_input = ui::dialog::Input {
                    focus: true,
                    ..Default::default()
                };
                self.dialog = Some(Dialog::Feed);
            }
            Action::CancelSelection => {
                self.selecting = false;
                self.selecting_delete = false;
                self.clear_selection();
                if let Some(view) = self.selection_return.take() {
                    self.apply(Action::Navigate(view), ctx);
                }
            }
            Action::ShowDetails { title, message } => {
                if let Some(view) = &self.native_reader {
                    view.focus_controls();
                }
                self.dialog = Some(Dialog::Details { title, message });
                ctx.request_repaint();
            }
            Action::SelectMagazine | Action::SelectDelete => {
                let deleting = matches!(action, Action::SelectDelete);
                self.selecting = !(self.selecting && self.selecting_delete == deleting);
                self.selecting_delete = deleting;
                self.clear_selection();
                if self.selecting && !deleting {
                    self.selection_return = (self.view != View::Inbox).then_some(self.view);
                } else if !self.selecting || deleting {
                    self.selection_return = None;
                }
                if !deleting && self.view != View::Inbox {
                    self.view = View::Inbox;
                    self.refresh();
                }
            }
            Action::ToggleSelection(id) => {
                if let Some(i) = self.selection.iter().position(|x| x == &id) {
                    self.selection.remove(i);
                    self.selection_metadata.remove(&id);
                } else {
                    if let Some(article) = self.rows.iter().find(|article| article.id == id) {
                        self.selection_metadata.insert(id.clone(), article.clone());
                        self.selection.push(id);
                    } else {
                        self.error = "The article list changed. Select the article again.".into();
                    }
                }
            }
            Action::Group(grouped) => {
                if self.view == View::Archive {
                    self.settings.archive_grouped = grouped;
                    self.settings.archive_scroll = 0.;
                    self.settings.archive.publication.clear();
                } else {
                    self.settings.inbox_grouped = grouped;
                    self.settings.inbox_scroll = 0.;
                    self.settings.inbox.publication.clear();
                }
                self.refresh();
            }
            Action::ConfirmDeleteMatching { query, expected } => {
                self.dialog = Some(Dialog::DeleteMatching { query, expected })
            }
            Action::ConfirmDelete(ids) => {
                if !ids.is_empty() {
                    self.dialog = Some(Dialog::Delete(ids));
                }
            }
            Action::DraftDialog => {
                let articles: Vec<_> = self
                    .selection
                    .iter()
                    .filter_map(|id| self.selection_metadata.get(id).cloned())
                    .collect();
                self.magazine_state
                    .start_manual(self.selection.clone(), &articles);
                self.selection_return = None;
                self.clear_selection();
                self.selecting = false;
                self.view = View::Magazines;
            }
            Action::FontSize(size) => {
                self.settings.reader_size = size;
                self.native_dirty = true;
                ctx.request_repaint();
            }
            Action::Appearance(appearance) => {
                self.settings.appearance = appearance;
                crate::theme::apply(ctx, appearance);
                for pdf in &self.pdfs {
                    pdf.set_appearance(appearance);
                }
                self.native_dirty = true;
                if let Err(error) = self.settings.save(&self.dirs.settings()) {
                    self.error = error.to_string();
                }
            }
            Action::ReaderFont(font) => {
                self.settings.reader_font = font;
                self.native_dirty = true;
                ctx.request_repaint();
            }
            Action::WhatsAppConnect(phone) => {
                if self.offline {
                    self.error =
                        "Launch without --offline after import verification to pair WhatsApp"
                            .into();
                } else if let Some(wa) = &self.whatsapp
                    && let Err(e) = wa.commands.try_send(WhatsAppCommand::Connect { phone })
                {
                    self.error = e.to_string();
                }
            }
            Action::WhatsAppDisconnect => {
                if let Some(wa) = &self.whatsapp
                    && let Err(e) = wa.commands.try_send(WhatsAppCommand::Disconnect)
                {
                    self.error = e.to_string();
                }
            }
            Action::Quit => {
                self.quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
    fn update_qr(&mut self, ctx: &egui::Context) {
        if self.qr_payload == self.whatsapp_view.qr {
            return;
        }
        self.qr_payload = self.whatsapp_view.qr.clone();
        self.qr = None;
        if let Some(payload) = &self.qr_payload {
            match qrcode::QrCode::new(payload) {
                Ok(code) => {
                    let w = code.width();
                    let scale = 4;
                    let quiet = 4;
                    let size = (w + quiet * 2) * scale;
                    let mut pixels = vec![egui::Color32::WHITE; size * size];
                    for y in 0..w {
                        for x in 0..w {
                            if code[(x, y)] == qrcode::Color::Dark {
                                for yy in 0..scale {
                                    for xx in 0..scale {
                                        pixels[((y + quiet) * scale + yy) * size
                                            + (x + quiet) * scale
                                            + xx] = egui::Color32::BLACK;
                                    }
                                }
                            }
                        }
                    }
                    self.qr = Some(ctx.load_texture(
                        "native-pairing",
                        egui::ColorImage::new([size, size], pixels),
                        egui::TextureOptions::NEAREST,
                    ));
                }
                Err(e) => self.error = format!("QR code could not be rendered: {e}"),
            }
        }
    }
    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        // Actions are applied after drawing. Hide native children immediately,
        // including the first modal frame, before the pure egui dialog takes focus.
        if let Some(view) = &self.native_reader {
            view.focus_controls();
            view.set_visible(false);
            view.set_font_picker_rect(
                None,
                ctx.input(|input| input.content_rect().size()),
                self.settings.reader_font,
            );
        }
        let description = match &dialog {
            Dialog::Delete(ids) => format!("Delete {} article(s) from your library?", ids.len()),
            Dialog::DeleteMatching { query, expected } => format!(
                "Delete {expected} matching articles from {}?",
                query.publication
            ),
            _ => String::new(),
        };
        let kind = match &dialog {
            Dialog::Save => ui::dialog::Kind::SaveLink,
            Dialog::Feed => ui::dialog::Kind::AddFeed,
            Dialog::Delete(_) | Dialog::DeleteMatching { .. } => ui::dialog::Kind::Delete {
                description: &description,
            },
            Dialog::Details { title, message } => ui::dialog::Kind::Details { title, message },
        };
        match ui::dialog::draw(ctx, kind, &mut self.dialog_input) {
            Some(ui::dialog::Decision::Submit) => {
                if matches!(dialog, Dialog::Save | Dialog::Feed)
                    && let Err(error) = crate::network::normalize_url(&self.dialog_input.value)
                {
                    self.dialog_input.error = if self.dialog_input.value.trim().is_empty() {
                        "Paste an HTTP or HTTPS address to continue.".into()
                    } else {
                        error.to_string()
                    };
                    self.dialog_input.focus = true;
                    ctx.request_repaint();
                    return;
                }
                let command = match dialog {
                    Dialog::Save => Command::Save(self.dialog_input.value.clone()),
                    Dialog::Feed => Command::AddFeed(self.dialog_input.value.clone()),
                    Dialog::DeleteMatching { query, expected } => {
                        Command::DeleteMatching { query, expected }
                    }
                    Dialog::Delete(ids) => {
                        self.selected = None;
                        self.body_generation += 1;
                        Command::Delete(ids)
                    }
                    Dialog::Details { .. } => return,
                };
                self.send(command);
                self.dialog = None;
                self.clear_selection();
                self.selecting = false;
                ctx.request_repaint();
            }
            Some(ui::dialog::Decision::Cancel) => {
                self.dialog = None;
                self.dialog_input.focus = false;
                ctx.request_repaint();
            }
            None => {}
        }
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while self.instance.events.try_recv().is_ok() {
            crate::native_menu::activate();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        for command in crate::native_menu::drain() {
            use crate::native_menu::Command as M;
            let action = match command {
                M::Show => {
                    crate::native_menu::activate();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    continue;
                }
                M::Quit => Action::Quit,
                M::Save => Action::SaveDialog,
                M::Feed => Action::FeedDialog,
                M::Inbox => Action::Navigate(View::Inbox),
                M::Archive => Action::Navigate(View::Archive),
                M::Feeds => Action::Navigate(View::Feeds),
                M::Magazines => Action::Navigate(View::Magazines),
                M::WhatsApp => Action::Navigate(View::WhatsApp),
                M::ImportExport => Action::Navigate(View::ImportExport),
                M::Cut => {
                    ctx.input_mut(|i| i.events.push(egui::Event::Cut));
                    continue;
                }
                M::Copy => {
                    ctx.input_mut(|i| i.events.push(egui::Event::Copy));
                    continue;
                }
                M::Paste => {
                    if let Some(text) = crate::native_menu::clipboard_text() {
                        ctx.input_mut(|i| i.events.push(egui::Event::Paste(text)));
                    }
                    continue;
                }
                M::SelectAll => {
                    ctx.input_mut(|i| {
                        i.events.push(egui::Event::Key {
                            key: egui::Key::A,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::COMMAND,
                        })
                    });
                    continue;
                }
                M::Search => {
                    if let Some(view) = &self.native_reader {
                        view.focus_controls();
                    }
                    ctx.memory_mut(|m| m.request_focus(egui::Id::new("library-search")));
                    continue;
                }
                M::DisplayPreferencesChanged => {
                    ctx.all_styles_mut(|style| {
                        style.animation_time = if crate::native_menu::reduce_motion() {
                            0.
                        } else {
                            0.12
                        }
                    });
                    continue;
                }
            };
            self.apply(action, ctx);
        }
        self.events(ctx);
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.settings.window_size = [rect.width(), rect.height()];
            self.settings.window_position = Some([rect.min.x, rect.min.y]);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Q)) {
            self.quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::N)) {
            self.apply(Action::SaveDialog, ctx);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let colors = crate::theme::palette(&ctx);
        if self.native_reader.is_none()
            && let Ok(handle) = frame.window_handle()
            && let RawWindowHandle::AppKit(handle) = handle.as_raw()
        {
            match unsafe { crate::rich_content::native::NativeRichView::attach(handle.ns_view) } {
                Ok(view) => {
                    let wake_ctx = ctx.clone();
                    view.set_wake(std::sync::Arc::new(move || wake_ctx.request_repaint()));
                    self.native_reader = Some(view);
                }
                Err(e) => self.error = e,
            }
        }
        if let Some(view) = &mut self.native_reader {
            self.native_dirty |= view.set_appearance(self.settings.appearance);
        }
        if self.native_dirty
            && let (Some(view), Some(body)) = (&mut self.native_reader, &self.selected)
        {
            let images = body.media.iter().cloned().collect();
            let document = crate::rich_content::RichDocument::parse(&body.html);
            if let Err(e) = view.set_document(
                &document,
                &images,
                self.settings.reader_size,
                self.settings.reader_font,
            ) {
                self.error = e;
            }
            let article = &body.article;
            let metadata = format!(
                "{}{} · {}",
                article
                    .author
                    .as_ref()
                    .map(|a| format!("{a} · "))
                    .unwrap_or_default(),
                article.reading_label(),
                article
                    .published_at
                    .as_ref()
                    .map(|d| format!("Published {}", crate::domain::date_label(d)))
                    .unwrap_or_else(|| format!(
                        "Saved {}",
                        crate::domain::date_label(&article.saved_at)
                    ))
            );
            let has_copy = !body.html.trim().is_empty();
            let notice = self.body_error.as_ref().or(article.error.as_ref()).map(|error| {
                crate::rich_content::native::ReaderNotice::Recovery {
                    title: if has_copy { "Your open reading copy is kept" } else { "Reading copy unavailable" }.into(),
                    summary: if has_copy { "A new copy could not be acquired. You can keep reading this copy, retry, or open the original." } else { "The publisher may be temporarily unavailable or its page may have changed. Retry, or open the original article." }.into(),
                    details: error.clone(),
                }
            }).or_else(|| {
                (article.status != CopyStatus::Saved).then(|| crate::rich_content::native::ReaderNotice::Recovery {
                    title: match article.status { CopyStatus::Partial => "Partial reading copy", CopyStatus::Pending => "Reading copy pending", CopyStatus::Failed => "Reading copy unavailable", CopyStatus::Saved => "Reading copy" }.into(),
                    summary: "This copy may not contain the complete article. Retry acquisition or open the original publisher page.".into(),
                    details: format!("The last acquisition was recorded as {}. Cached copies are temporary and publishers can change their pages.", article.status.as_str()),
                })
            });
            if let Err(error) = view.set_header(
                crate::rich_content::native::ReaderHeader {
                    article_id: article.id.clone(),
                    publication: article.site.clone(),
                    title: article.title.clone(),
                    metadata,
                    read: article.read_at.is_some(),
                    archived: article.archived_at.is_some(),
                    fetch_info: self
                        .fetched_at
                        .as_ref()
                        .map(|at| format!("Fetched {}", crate::domain::date_label(at))),
                    notice,
                },
                self.settings.reader_font,
            ) {
                self.error = error;
            }
            self.native_dirty = false;
        }
        let mut show_native = false;
        let mut font_picker_rect = None;
        let mut actions = vec![];
        let mut magazine_actions = vec![];
        if !self.error.is_empty() || !self.notice.is_empty() {
            // Reserve real layout space so native reader children never cover
            // feedback, and feedback never hides the article or takes focus.
            egui::Panel::bottom("library-feedback")
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(colors.soft)
                        .inner_margin(egui::Margin::symmetric(16, 10)),
                )
                .show(ui, |ui| {
                    if !self.error.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(feedback_summary(&self.error))
                                    .color(colors.danger),
                            );
                            if ui::controls::action(ui, "Details", ui::controls::ButtonKind::Quiet)
                                .clicked()
                            {
                                actions.push(Action::ShowDetails {
                                    title: "Operation details".into(),
                                    message: self.error.clone(),
                                });
                            }
                            if ui::controls::icon_button(
                                ui,
                                ui::icons::Icon::Close,
                                "Dismiss error",
                                false,
                            )
                            .clicked()
                            {
                                self.error.clear();
                            }
                        });
                    }
                    if !self.notice.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(feedback_summary(&self.notice))
                                    .color(colors.green),
                            );
                            if (self.notice.chars().count() > 180
                                || self.notice.lines().count() > 1)
                                && ui::controls::action(
                                    ui,
                                    "Details",
                                    ui::controls::ButtonKind::Quiet,
                                )
                                .clicked()
                            {
                                actions.push(Action::ShowDetails {
                                    title: "Operation details".into(),
                                    message: self.notice.clone(),
                                });
                            }
                            if ui::controls::icon_button(
                                ui,
                                ui::icons::Icon::Close,
                                "Dismiss notice",
                                false,
                            )
                            .clicked()
                            {
                                self.notice.clear();
                            }
                        });
                    }
                });
        }
        egui::Panel::left("navigation")
            .exact_size(64.)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(colors.sidebar))
            .show(ui, |ui| ui::rail::draw(ui, self.view, &mut actions));
        if matches!(self.view, View::Inbox | View::Archive) {
            let width = if ui.available_width() + 64. < 1100. {
                300.
            } else {
                340.
            };
            let scroll = if self.view == View::Archive {
                self.settings.archive_scroll
            } else {
                self.settings.inbox_scroll
            };
            let mut new_scroll = scroll;
            egui::Panel::left("articles")
                .exact_size(width)
                .resizable(false)
                .frame(egui::Frame::NONE.fill(colors.article))
                .show(ui, |ui| {
                    new_scroll = ui::library::draw(
                        ui,
                        ui::library::State {
                            view: self.view,
                            query: self.query(),
                            rows: &self.rows,
                            total: self.matching_count,
                            library_total: self.counts.all,
                            grouped: if self.view == View::Archive {
                                self.settings.archive_grouped
                            } else {
                                self.settings.inbox_grouped
                            },
                            logos: &self.logos,
                            publications: &self.publications,
                            selected: self.selected.as_ref().map(|a| &a.article.id),
                            selection: &self.selection,
                            selecting: self.selecting,
                            selecting_delete: self.selecting_delete,
                            loading: self.loading,
                            has_more: self.has_more,
                            scroll,
                        },
                        &mut actions,
                    );
                });
            if self.view == View::Archive {
                self.settings.archive_scroll = new_scroll;
            } else {
                self.settings.inbox_scroll = new_scroll;
            }
            let mut reader_layout = ui::reader::ReaderLayout::default();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(colors.reader))
                .show(ui, |ui| {
                    reader_layout = ui::reader::draw(
                        ui,
                        ui::reader::ReaderView {
                            article: self.selected.as_ref().map(|a| &a.article),
                            reader: &self.reader,
                            size: self.settings.reader_size,
                            font: self.settings.reader_font,
                            loading: self.body_loading,
                            error: self.body_error.as_deref(),
                            fetched_at: self.fetched_at.as_deref(),
                            has_copy: self
                                .selected
                                .as_ref()
                                .is_some_and(|body| !body.html.trim().is_empty()),
                            native: self.native_reader.is_some(),
                        },
                        &mut actions,
                    );
                });
            font_picker_rect = reader_layout.font_picker;
            if let (Some(rect), Some(view)) = (reader_layout.body, &self.native_reader)
                && self.dialog.is_none()
                && rect.height() > 0.
            {
                view.set_rect(rect, ctx.input(|i| i.content_rect().size()));
                show_native = true;
            }
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(colors.paper).inner_margin({
                    let side = if ctx.content_rect().width() < 1050. {
                        28
                    } else {
                        48
                    };
                    egui::Margin {
                        left: side,
                        right: side,
                        top: ui::TITLEBAR_INSET as i8 + 20,
                        bottom: 36,
                    }
                }))
                .show(ui, |ui| match self.view {
                    View::Feeds => ui::secondary::feeds(ui, &self.feeds, &mut actions),
                    View::Magazines => ui::magazine::draw(
                        ui,
                        &self.magazines,
                        &mut self.magazine_state,
                        &mut magazine_actions,
                    ),
                    View::WhatsApp => ui::whatsapp::draw(
                        ui,
                        &self.whatsapp_view,
                        &mut self.phone,
                        self.offline,
                        self.qr.as_ref(),
                        &mut actions,
                    ),
                    View::ImportExport => ui::import_export::draw(
                        ui,
                        &self.imports,
                        self.transfer_activity.map(|(_, kind)| kind),
                        &mut actions,
                    ),
                    _ => {}
                });
        }
        if let Some(view) = &self.native_reader {
            view.set_visible(show_native);
            view.set_font_picker_rect(
                font_picker_rect.filter(|_| self.dialog.is_none()),
                ctx.input(|i| i.content_rect().size()),
                self.settings.reader_font,
            );
        }
        for action in magazine_actions {
            use ui::magazine::Action as A;
            match action {
                A::Command(command) => {
                    if matches!(&command, crate::magazine::Command::Open(_)) {
                        self.magazine_state.open();
                    } else if matches!(&command, crate::magazine::Command::Save(_)) {
                        self.magazine_state.loading = true;
                    }
                    self.send(Command::Magazine(command));
                }
                A::NewManual => self.apply(Action::SelectMagazine, &ctx),
                A::OpenPdf(id) => {
                    self.magazine_state.opened_pdf_sha256 = None;
                    self.send(Command::Pdf(id));
                }
                A::ExportRequest(request) => {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("cover-request.json")
                        .save_file()
                    {
                        self.send(Command::ExportCoverRequest { request, path });
                    }
                }
                A::ChoosePdfRemoval { id, sha256, title } => {
                    let filename: String = title
                        .chars()
                        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
                        .take(100)
                        .collect();
                    let filename = if filename.trim().is_empty() {
                        "Read Later edition"
                    } else {
                        filename.trim()
                    };
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Export verified PDF before removing its local copy")
                        .set_file_name(format!("{filename}.pdf"))
                        .add_filter("PDF", &["pdf"])
                        .save_file()
                    {
                        self.send(Command::Magazine(
                            crate::magazine::Command::ExportAndRemovePdf { id, sha256, path },
                        ));
                    }
                }
                A::PickCovers {
                    magazine_id,
                    request_id,
                    input_sha256,
                } => {
                    if let Some(front) = rfd::FileDialog::new()
                        .set_title("Choose original front painting")
                        .add_filter("Raster painting", &["png", "jpg", "jpeg"])
                        .pick_file()
                        && let Some(back) = rfd::FileDialog::new()
                            .set_title("Choose original back painting")
                            .add_filter("Raster painting", &["png", "jpg", "jpeg"])
                            .pick_file()
                    {
                        self.send(Command::CoverFiles {
                            magazine_id,
                            request_id,
                            input_sha256,
                            front,
                            back,
                        });
                    }
                }
                A::PickRepair(input) => {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Choose sourced original image")
                        .add_filter("Raster source", &["png", "jpg", "jpeg"])
                        .pick_file()
                    {
                        let context = input.context.clone();
                        match self.backend.send(Command::RepairFile { input, path }) {
                            Ok(()) => {
                                if let Some(context) = context {
                                    self.magazine_state.repair_started(&context);
                                    ctx.request_repaint();
                                }
                            }
                            Err(message) => self.error = message,
                        }
                    }
                }
                A::Close => self.magazine_state = Default::default(),
            }
        }
        if let Some(view) = &self.native_reader {
            if let Some(font) = view.take_font_choice() {
                actions.push(Action::ReaderFont(font));
            }
            for action in view.take_actions() {
                use crate::rich_content::native::ReaderActionKind as R;
                actions.push(match action.kind {
                    R::ToggleRead(read) => Action::Command(Command::Read {
                        id: action.article_id,
                        read,
                    }),
                    R::Archive(archived) => Action::Command(Command::Archive {
                        id: action.article_id,
                        archived,
                    }),
                    R::Delete => Action::ConfirmDelete(vec![action.article_id]),
                    R::Refetch => Action::Command(Command::Refetch(action.article_id)),
                    R::Details { title, message } => Action::ShowDetails { title, message },
                });
            }
        }
        for action in actions {
            self.apply(action, &ctx);
        }
        self.dialog(&ctx);
        let controls_focus = ctx.memory(|memory| memory.focused());
        if ctx.egui_wants_keyboard_input()
            && (controls_focus != self.controls_keyboard_focus
                || ctx.input(|input| input.pointer.any_pressed()))
            && let Some(view) = &self.native_reader
        {
            view.focus_controls();
        }
        // Native text clicks do not enter egui's pointer stream. Keeping the
        // last egui focus lets native selection/copy remain active until the
        // user actually focuses or clicks an egui control again.
        self.controls_keyboard_focus = controls_focus;
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.settings.save(&self.dirs.settings());
        if let Some(production) = self.production.take() {
            production.shutdown();
        }
        if let Some(intake) = self.intake.take() {
            intake.shutdown();
        }
        if let Some(wa) = self.whatsapp.take() {
            self.runtime.block_on(wa.shutdown());
        }
        self.backend.shutdown();
    }
}
fn feedback_summary(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or(message);
    let mut summary: String = first_line.chars().take(180).collect();
    if message.chars().count() > summary.chars().count() {
        summary.push('…');
    }
    summary
}

/// Apply the same native typography and control theme to the app and preview gallery.
pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, path, index) in [
        (
            "Georgia",
            "/System/Library/Fonts/Supplemental/Georgia.ttf",
            0,
        ),
        (
            "Georgia-Bold",
            "/System/Library/Fonts/Supplemental/Georgia Bold.ttf",
            0,
        ),
        (
            "Georgia-Italic",
            "/System/Library/Fonts/Supplemental/Georgia Italic.ttf",
            0,
        ),
        (
            "Baskerville",
            "/System/Library/Fonts/Supplemental/Baskerville.ttc",
            0,
        ),
        ("Avenir Next", "/System/Library/Fonts/Avenir Next.ttc", 7),
        (
            "Helvetica Neue",
            "/System/Library/Fonts/HelveticaNeue.ttc",
            0,
        ),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            fonts
                .font_data
                .insert(name.into(), std::sync::Arc::new(data));
            fonts
                .families
                .insert(egui::FontFamily::Name(name.into()), vec![name.into()]);
        }
    }
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        fonts.font_data.insert(
            "System Sans".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "System Sans".into());
    }
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/Supplemental/Arial Bold.ttf") {
        fonts.font_data.insert(
            "System Semibold".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts.families.insert(
            egui::FontFamily::Name("System Semibold".into()),
            vec!["System Semibold".into(), "System Sans".into()],
        );
    }
    ctx.set_fonts(fonts);
    crate::theme::configure(ctx, crate::native_menu::reduce_motion());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_change_signals_queues_one_reload() {
        let mut reload = PendingReload::default();
        for _ in 0..64 {
            reload.list = true;
            reload.selected = true;
        }
        let mut queued = 0;
        reload.flush(|_| {
            queued += 1;
            true
        });
        assert_eq!(queued, 3);
        assert!(!reload.pending());
    }

    #[test]
    fn a_full_queue_keeps_the_reload_owed() {
        let mut reload = PendingReload {
            list: true,
            ..Default::default()
        };
        reload.flush(|_| false);
        assert!(reload.list);
        reload.flush(|step| matches!(step, ReloadStep::List));
        assert!(!reload.list);
        assert!(reload.summary);
        reload.flush(|_| true);
        assert!(!reload.pending());
    }
}
