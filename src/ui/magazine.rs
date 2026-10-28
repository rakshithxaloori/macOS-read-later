//! Magazine views emit actions; database, files and production belong to background owners.
use super::{
    controls::{self, ButtonKind},
    icons::Icon,
};
use crate::{
    domain::{Article, ArticleId, Magazine, MagazineId},
    magazine::*,
};
use egui::{RichText, Ui};
#[derive(Clone, Debug)]
pub enum Action {
    Command(Command),
    NewManual,
    OpenPdf(MagazineId),
    ExportRequest(CoverRequest),
    ChoosePdfRemoval {
        id: MagazineId,
        sha256: String,
        title: String,
    },
    PickCovers {
        magazine_id: MagazineId,
        request_id: String,
        input_sha256: String,
    },
    PickRepair(RepairInput),
    Close,
}
#[derive(Clone, Debug)]
struct RepairFailure {
    request_id: uuid::Uuid,
    article_id: ArticleId,
    detail: String,
}
#[derive(Default)]
pub struct State {
    pub edition: Option<Edition>,
    pub plans: Option<Plans>,
    pub policies: Option<Policies>,
    publication_exclusion: String,
    cover_requested: std::collections::HashSet<MagazineId>,
    cover_rasters: std::collections::HashMap<MagazineId, CoverRaster>,
    cover_textures: std::collections::HashMap<MagazineId, egui::TextureHandle>,
    pub editing: Option<DraftInput>,
    pub loading: bool,
    pub progress: Option<String>,
    acknowledged: bool,
    repair_article: usize,
    edit_generation: uuid::Uuid,
    completed_repairs: std::collections::HashSet<uuid::Uuid>,
    pending_repairs: std::collections::HashMap<uuid::Uuid, RepairContext>,
    repair_error: Option<RepairFailure>,
    repair_anchor: String,
    repair_alt: String,
    repair_note: String,
    repair_source: String,
    repair_image_url: String,
    repair_label: String,
    pub cover_request: Option<CoverRequest>,
    delete_confirm: bool,
    delete_acknowledged: bool,
    remove_pdf_confirm: Option<(MagazineId, String)>,
    pub opened_pdf_sha256: Option<String>,
    manual_articles: std::collections::HashMap<ArticleId, Article>,
    edit_baseline: Option<serde_json::Value>,
    exclusions_open: bool,
    policies_pending: bool,
    policy_filter: String,
    repair_open: Option<ArticleId>,
    preview_generation: u64,
    preview_requested: Option<(MagazineId, u64)>,
    previews: std::collections::HashMap<String, AssetPreview>,
    preview_textures: std::collections::HashMap<String, egui::TextureHandle>,
    provenance: Option<String>,
}
impl State {
    pub fn open(&mut self) {
        self.edit_generation = uuid::Uuid::new_v4();
        self.completed_repairs.clear();
        self.pending_repairs.clear();
        self.repair_error = None;
        self.progress = None;
        self.loading = true;
        self.opened_pdf_sha256 = None;
        self.editing = None;
        self.edition = None;
        self.plans = None;
        self.remove_pdf_confirm = None;
    }
    pub fn start_manual(&mut self, ids: Vec<ArticleId>, articles: &[Article]) {
        self.edit_generation = uuid::Uuid::new_v4();
        self.completed_repairs.clear();
        self.pending_repairs.clear();
        self.repair_error = None;
        self.manual_articles = articles
            .iter()
            .filter(|a| ids.contains(&a.id))
            .map(|a| (a.id.clone(), a.clone()))
            .collect();
        self.edit_baseline = None;
        self.repair_open = None;
        self.progress = None;
        self.edition = None;
        self.plans = None;
        self.editing = Some(DraftInput {
            id: None,
            title: "A longer read".into(),
            subtitle: String::new(),
            paper_size: PaperSize::Letter,
            article_ids: ids,
            refresh_copies: false,
            design: Design::default(),
            presentation: Default::default(),
        });
    }
    pub fn receive(&mut self, event: Event) {
        if let Event::Previews {
            id,
            generation,
            assets,
            provenance,
        } = event
        {
            let current = self
                .editing
                .as_ref()
                .and_then(|input| input.id.as_ref())
                .or_else(|| self.edition.as_ref().map(|e| &e.issue.id));
            if current == Some(&id) && generation == self.preview_generation {
                self.previews = assets
                    .into_iter()
                    .map(|asset| (asset.key.clone(), asset))
                    .collect();
                self.preview_textures.clear();
                self.provenance = provenance;
            }
            return;
        }
        if let Event::Repair(RepairResult { context, asset }) = event {
            if let Some(context) = context
                && self.take_pending_repair(&context)
            {
                if let Some(input) = &mut self.editing
                    && input.article_ids.contains(&context.article_id)
                {
                    input
                        .presentation
                        .entry(context.article_id.clone())
                        .or_default()
                        .insertions
                        .push(ImageInsertion {
                            asset_id: asset.id,
                            sha256: asset.sha256,
                            after_text: context.after_text.clone(),
                            alt: context.alt.clone(),
                            note: context.note.clone(),
                            source_url: asset.source_url,
                            image_url: asset.image_url,
                            retrieved_at: asset.retrieved_at,
                        });
                }
                self.completed_repairs.insert(context.request_id);
                if self
                    .repair_error
                    .as_ref()
                    .is_some_and(|error| error.request_id == context.request_id)
                {
                    self.repair_error = None;
                }
                self.invalidate_previews();
            }
            return;
        }
        if let Event::RepairFailed { context, message } = event {
            if let Some(context) = context
                && self.take_pending_repair(&context)
            {
                self.repair_error = Some(RepairFailure {
                    request_id: context.request_id,
                    article_id: context.article_id.clone(),
                    detail: message,
                });
            }
            return;
        }
        let waiting = self.loading;
        self.loading = false;
        match event {
            Event::Cover { id, raster } => {
                if let Some(raster) = raster {
                    self.cover_rasters.insert(id, raster);
                }
            }
            Event::Policies(policies) => {
                self.policies_pending = false;
                self.policies = Some(policies);
            }
            Event::Plans(plans) => {
                self.progress = None;
                self.plans = Some(plans);
                self.edition = None;
            }
            Event::Edition(edition) => {
                self.invalidate_previews();
                self.progress = None;
                self.cover_request = edition.cover_request.clone();
                if waiting
                    || self.editing.as_ref().and_then(|d| d.id.as_ref()) != Some(&edition.issue.id)
                {
                    self.editing = None;
                }
                if edition
                    .production
                    .as_ref()
                    .is_some_and(|p| p.state == ProductionState::Done)
                {
                    self.cover_requested.remove(&edition.issue.id);
                    self.cover_textures.remove(&edition.issue.id);
                    self.cover_rasters.remove(&edition.issue.id);
                }
                self.edition = Some(*edition);
            }
            Event::Covers(request) => {
                self.invalidate_previews();
                self.cover_request = Some(request);
            }
            Event::Repair(_) | Event::RepairFailed { .. } => {
                unreachable!("repair scoped before mutation state")
            }
            Event::Deleted(_) => {
                self.edition = None;
                self.editing = None;
                self.cover_request = None;
            }
            Event::Previews { .. } => unreachable!("preview handled before mutation state"),
            Event::Changed => {
                self.plans = None;
                self.policies = None;
            }
        }
    }
    /// Called only after a chosen file has been accepted by the backend queue.
    pub fn repair_started(&mut self, context: &RepairContext) -> bool {
        if !self.repair_context_is_current(context)
            || !self
                .editing
                .as_ref()
                .is_some_and(|input| input.article_ids.contains(&context.article_id))
            || self.completed_repairs.contains(&context.request_id)
            || self.pending_repairs.contains_key(&context.request_id)
        {
            return false;
        }
        self.pending_repairs
            .insert(context.request_id, context.clone());
        if self
            .repair_error
            .as_ref()
            .is_some_and(|error| error.article_id == context.article_id)
        {
            self.repair_error = None;
        }
        true
    }
    fn repair_context_is_current(&self, context: &RepairContext) -> bool {
        context.edit_generation == self.edit_generation
            && !context.edit_generation.is_nil()
            && self
                .editing
                .as_ref()
                .is_some_and(|input| input.id.as_ref() == Some(&context.magazine_id))
    }
    fn take_pending_repair(&mut self, context: &RepairContext) -> bool {
        if !self.repair_context_is_current(context)
            || self.pending_repairs.get(&context.request_id) != Some(context)
        {
            return false;
        }
        self.pending_repairs.remove(&context.request_id);
        true
    }
    fn relevant_pending_repairs(&self, input: &DraftInput) -> usize {
        self.pending_repairs
            .values()
            .filter(|context| {
                context.edit_generation == self.edit_generation
                    && input.id.as_ref() == Some(&context.magazine_id)
            })
            .count()
    }
    fn can_save(&self, input: &DraftInput) -> bool {
        !input.article_ids.is_empty() && self.relevant_pending_repairs(input) == 0
    }
    fn invalidate_previews(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview_requested = None;
        self.previews.clear();
        self.preview_textures.clear();
        self.provenance = None;
    }
    pub fn begin_edit(&mut self) {
        self.progress = None;
        if let Some(edition) = &self.edition
            && edition.issue.finalized_at.is_none()
        {
            self.edit_generation = uuid::Uuid::new_v4();
            self.completed_repairs.clear();
            self.pending_repairs.clear();
            self.repair_error = None;
            let issue = &edition.issue;
            let mut design = issue.design.clone();
            design.front_asset = None;
            design.back_asset = None;
            design.companion_asset = None;
            self.editing = Some(DraftInput {
                id: Some(issue.id.clone()),
                title: issue.title.clone(),
                subtitle: issue.subtitle.clone(),
                paper_size: issue.paper_size,
                article_ids: issue
                    .articles
                    .iter()
                    .map(|a| a.article_id.clone())
                    .collect(),
                refresh_copies: false,
                design,
                presentation: issue
                    .articles
                    .iter()
                    .map(|a| (a.article_id.clone(), a.presentation.clone()))
                    .collect(),
            });
            self.edit_baseline = self.editing.as_ref().map(draft_signature);
        }
    }
    pub fn is_open(&self) -> bool {
        self.edition.is_some() || self.editing.is_some() || self.plans.is_some() || self.loading
    }
}
pub fn draw(ui: &mut Ui, issues: &[Magazine], state: &mut State, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    if state.loading {
        heading(ui, "FROM SCREEN TO PAPER", "Magazines.");
        controls::panel(ui).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(state.progress.as_deref().unwrap_or("Opening your edition…"));
            });
        });
        return;
    }
    if let Some(progress) = &state.progress {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(progress);
        });
    }
    if state.editing.is_some() {
        editor(ui, state, actions);
        return;
    }
    if state.edition.is_some() {
        review(ui, state, actions);
        return;
    }
    egui::ScrollArea::vertical().id_salt("magazine-list").show(ui, |ui| {
        heading(ui, "FROM SCREEN TO PAPER", "Magazines.");
        ui.label(RichText::new("Gather a few good reads. Give them a life on paper.").size(13.).color(colors.muted));
        ui.add_space(18.);
        ui.horizontal_wrapped(|ui| {
            if controls::action(ui, "Create a magazine", ButtonKind::Primary).clicked() { actions.push(Action::NewManual); }
            if controls::action(ui, "Suggest magazines", ButtonKind::Secondary).clicked() { actions.push(Action::Command(Command::Plans)); }
            if controls::action(ui, "Exclusions", ButtonKind::Quiet).clicked() { state.exclusions_open = true; state.policies_pending = true; actions.push(Action::Command(Command::Policies)); }
        });
        ui.add_space(22.);
        if let Some(plans) = &state.plans { proposals(ui, plans, actions); ui.add_space(22.); }
        ui.separator(); ui.add_space(22.);
        if issues.is_empty() {
            controls::empty(ui, Icon::Newspaper, "A shelf waiting for your first edition", "Choose a few articles or ask for a suggestion. Shape the pages, add paired cover paintings, and review the exact PDF before finalizing.");
            return;
        }
        let columns = ((ui.available_width() + 24.) / 310.).floor().clamp(1., 3.) as usize;
        let width = ((ui.available_width() - 24. * (columns - 1) as f32) / columns as f32).min(340.);
        egui::Grid::new("magazine-covers-grid")
            .num_columns(columns)
            .spacing(egui::vec2(24., 28.))
            .show(ui, |ui| {
                for (index, issue) in issues.iter().enumerate() {
                    request_cover(ui, state, &issue.id, actions);
                    let card = egui::Frame::new()
                        .fill(colors.reader)
                        .corner_radius(9)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            // Grid cells inherit a horizontal layout. A card needs
                            // finite-width vertical flow for its cover and labels.
                            let content_width = (width - 20.).max(80.);
                            ui.set_width(content_width);
                            ui.vertical(|ui| {
                                ui.set_width(content_width);
                                cover(
                                    ui,
                                    state,
                                    &issue.id,
                                    &issue.title,
                                    issue.issue_number,
                                    content_width,
                                    Some(issue.paper_size),
                                );
                                ui.add_space(12.);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&issue.title)
                                            .size(15.)
                                            .strong()
                                            .color(colors.ink),
                                    )
                                    .wrap(),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "Vol. {} · {}",
                                            issue.issue_number,
                                            crate::domain::date_label(&issue.created_at),
                                        ))
                                        .size(11.)
                                        .color(colors.muted),
                                    );
                                    controls::status(
                                        ui,
                                        if issue.finalized_at.is_some() {
                                            controls::Status::Finalized
                                        } else {
                                            controls::Status::Draft
                                        },
                                    );
                                    if let Some(pages) = issue.page_count {
                                        ui.label(
                                            RichText::new(format!("{pages} pages"))
                                                .size(11.)
                                                .color(colors.muted),
                                        );
                                    }
                                });
                                ui.add_space(4.);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Open edition")
                                            .size(12.)
                                            .color(colors.green),
                                    );
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(15., 15.),
                                        egui::Sense::hover(),
                                    );
                                    super::icons::draw(ui, rect, Icon::External, colors.green);
                                });
                            });
                        });
                    let response = ui.interact(
                        card.response.rect,
                        ui.id().with(("open-edition", &issue.id.0)),
                        egui::Sense::click(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("Open edition {}", issue.title),
                        )
                    });
                    if response.hovered() {
                        ui.painter().rect_stroke(
                            response.rect,
                            9,
                            egui::Stroke::new(1., colors.green.gamma_multiply(0.5)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    controls::focus(ui, &response);
                    if response.clicked() {
                        state.loading = true;
                        actions.push(Action::Command(Command::Open(issue.id.clone())));
                    }
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
    });
    if state.exclusions_open {
        exclusions(ui, state, actions);
    }
}
fn heading(ui: &mut Ui, eyebrow: &str, title: &str) {
    let colors = crate::theme::palette(ui.ctx());
    ui.label(RichText::new(eyebrow).size(10.).color(colors.muted));
    ui.add_space(6.);
    ui.label(
        RichText::new(title)
            .font(egui::FontId::new(
                42.,
                egui::FontFamily::Name("Georgia".into()),
            ))
            .color(colors.ink),
    );
}
fn request_cover(ui: &Ui, state: &mut State, id: &MagazineId, actions: &mut Vec<Action>) {
    if state.cover_requested.insert(id.clone()) {
        actions.push(Action::Command(Command::Cover(id.clone())));
    }
    if let Some(raster) = state.cover_rasters.remove(id) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [raster.width as usize, raster.height as usize],
            &raster.rgba,
        );
        let texture = ui.ctx().load_texture(
            format!("magazine-cover-{}", id.0),
            image,
            egui::TextureOptions::LINEAR,
        );
        state.cover_textures.insert(id.clone(), texture);
    }
}
fn request_previews(ui: &Ui, state: &mut State, id: &MagazineId, actions: &mut Vec<Action>) {
    let request = (id.clone(), state.preview_generation);
    if state.preview_requested.as_ref() != Some(&request) {
        state.preview_requested = Some(request.clone());
        actions.push(Action::Command(Command::Previews {
            id: request.0,
            generation: request.1,
        }));
    }
    for (key, asset) in &mut state.previews {
        if let Some(raster) = asset.raster.take() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [raster.width as usize, raster.height as usize],
                &raster.rgba,
            );
            state.preview_textures.insert(
                key.clone(),
                ui.ctx().load_texture(
                    format!("draft-preview-{}-{key}", id.0),
                    image,
                    egui::TextureOptions::LINEAR,
                ),
            );
        }
    }
}
fn asset_well(ui: &mut Ui, state: &State, key: &str, label: &str, height: f32) {
    let colors = crate::theme::palette(ui.ctx());
    let width = ui.available_width().max(40.);
    if let Some(texture) = state.preview_textures.get(key) {
        let source = texture.size_vec2();
        let scale = (width / source.x).min(height / source.y);
        ui.allocate_ui_with_layout(
            egui::vec2(width, height),
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
            |ui| {
                ui.add(
                    egui::Image::new(texture)
                        .alt_text(label)
                        .fit_to_exact_size(source * scale),
                );
            },
        );
    } else {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        ui.painter().rect_filled(rect, 7, colors.sidebar);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if state.previews.contains_key(key) {
                "Preview unavailable"
            } else if key.starts_with("cover:") {
                "Waiting for original"
            } else {
                "Thumbnail not loaded"
            },
            egui::FontId::proportional(12.),
            colors.muted,
        );
    }
    if let Some(asset) = state.previews.get(key) {
        if asset.verified {
            controls::status(ui, controls::Status::Verified);
        } else {
            controls::status(ui, controls::Status::PreviewUnavailable);
        }
        if let Some(error) = &asset.error {
            ui.label(RichText::new(error).size(11.).color(colors.muted));
        }
        ui.collapsing("Image evidence", |ui| {
            ui.label(&asset.label);
            ui.label(format!("SHA-256: {}", asset.sha256));
        });
    }
}
fn cover(
    ui: &mut Ui,
    state: &State,
    id: &MagazineId,
    title: &str,
    number: u32,
    width: f32,
    paper: Option<PaperSize>,
) {
    let colors = crate::theme::Appearance::Light.palette();
    if let Some(texture) = state.cover_textures.get(id) {
        let size = texture.size_vec2();
        ui.add(
            egui::Image::new(texture)
                .alt_text(format!("Cover of {title}"))
                .fit_to_exact_size(egui::vec2(width, width * size.y / size.x)),
        );
    } else {
        let ratio = paper
            .map(|p| {
                let (w, h) = p.points();
                (h / w) as f32
            })
            .unwrap_or(22. / 17.);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, width * ratio), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 3, egui::Color32::from_rgb(242, 238, 228));
        ui.painter().rect_stroke(
            rect,
            3,
            egui::Stroke::new(1., colors.divider),
            egui::StrokeKind::Inside,
        );
        let inset = (width * 0.08).max(10.);
        ui.painter().text(
            rect.min + egui::vec2(inset, inset),
            egui::Align2::LEFT_TOP,
            format!("ISSUE {number:02}"),
            egui::FontId::proportional(10.),
            colors.muted,
        );
        let text = ui.painter().layout(
            title.into(),
            egui::FontId::new(
                (width * 0.09).clamp(14., 26.),
                egui::FontFamily::Name("Georgia".into()),
            ),
            colors.ink,
            width - inset * 2.,
        );
        ui.painter().with_clip_rect(rect.shrink(inset)).galley(
            rect.min + egui::vec2(inset, inset + 22.),
            text,
            colors.ink,
        );
        ui.painter().line_segment(
            [
                rect.left_bottom() + egui::vec2(inset, -inset * 2.),
                rect.right_bottom() + egui::vec2(-inset, -inset * 2.),
            ],
            egui::Stroke::new(2., colors.green),
        );
    }
}
fn exclusions(ui: &mut Ui, state: &mut State, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    if state.policies.is_none() && !state.policies_pending {
        state.policies_pending = true;
        actions.push(Action::Command(Command::Policies));
    }
    let policies = state.policies.clone();
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("magazine-exclusions"))
        .frame(controls::panel(ui))
        .show(ui.ctx(), |ui| {
            ui.set_width((ui.ctx().content_rect().width() - 96.).clamp(240., 560.));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Excluded from suggestions")
                        .size(21.)
                        .strong(),
                );
                if controls::icon_button(ui, Icon::Close, "Close exclusions", false).clicked() {
                    close = true;
                }
            });
            ui.label(
                RichText::new("Exclusions affect future suggestions. Your library stays intact.")
                    .color(colors.muted),
            );
            controls::field(
                ui,
                &mut state.policy_filter,
                "Search articles or publication hostnames",
            );
            ui.add_space(10.);
            let filter = state.policy_filter.trim().to_lowercase();
            egui::ScrollArea::vertical()
                .id_salt("exclusion-results")
                .max_height((ui.ctx().content_rect().height() - 430.).clamp(80., 360.))
                .show(ui, |ui| {
                    if let Some(policies) = &policies {
                        let mut count = 0;
                        for (id, title) in &policies.articles {
                            if !title.to_lowercase().contains(&filter) {
                                continue;
                            }
                            count += 1;
                            ui.horizontal_wrapped(|ui| {
                                ui.label(title);
                                if controls::action(ui, "Allow article", ButtonKind::Quiet)
                                    .clicked()
                                {
                                    actions.push(Action::Command(Command::Exclude {
                                        id: id.clone(),
                                        excluded: false,
                                    }));
                                }
                            });
                        }
                        for hostname in &policies.publications {
                            if !hostname.contains(&filter) {
                                continue;
                            }
                            count += 1;
                            ui.horizontal_wrapped(|ui| {
                                ui.label(hostname);
                                if controls::action(ui, "Allow publication", ButtonKind::Quiet)
                                    .clicked()
                                {
                                    actions.push(Action::Command(Command::ExcludePublication {
                                        hostname: hostname.clone(),
                                        excluded: false,
                                    }));
                                }
                            });
                        }
                        if count == 0 {
                            ui.label(if filter.is_empty() {
                                "No exclusions."
                            } else {
                                "No exclusions match this search."
                            });
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading exclusions…");
                        });
                    }
                });
            ui.separator();
            ui.label("Exclude a publication");
            controls::field(
                ui,
                &mut state.publication_exclusion,
                "Publication hostname, e.g. example.com",
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(!state.publication_exclusion.trim().is_empty(), |ui| {
                        controls::action(ui, "Exclude publication", ButtonKind::Secondary)
                    })
                    .inner
                    .clicked()
                {
                    actions.push(Action::Command(Command::ExcludePublication {
                        hostname: state.publication_exclusion.trim().into(),
                        excluded: true,
                    }));
                }
                if controls::action(ui, "Done", ButtonKind::Primary).clicked() {
                    close = true;
                }
            });
        });
    if close || modal.should_close() {
        state.exclusions_open = false;
    }
}
fn proposals(ui: &mut Ui, plans: &Plans, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    ui.label(RichText::new("A few possible editions").size(21.).strong());
    ui.label(
        RichText::new(format!(
            "{} eligible unread articles · whole articles only",
            plans.eligible_count
        ))
        .color(colors.muted),
    );
    ui.add_space(12.);
    for proposal in &plans.proposals {
        controls::panel(ui).show(ui, |ui| {
            ui.label(RichText::new(&proposal.title).font(egui::FontId::new(
                25.,
                egui::FontFamily::Name("Georgia".into()),
            )));
            ui.label(&proposal.description);
            ui.horizontal_wrapped(|ui| {
                controls::badge(
                    ui,
                    &format!("{} articles", proposal.articles.len()),
                    colors.green,
                );
                controls::badge(ui, &format!("{} words", proposal.word_count), colors.muted);
            });
            ui.add_space(10.);
            for article in &proposal.articles {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&article.title);
                    ui.label(
                        RichText::new(format!("{} words", article.word_count))
                            .size(11.)
                            .color(colors.muted),
                    );
                    if controls::action(ui, "Exclude", ButtonKind::Quiet).clicked() {
                        actions.push(Action::Command(Command::Exclude {
                            id: article.id.clone(),
                            excluded: true,
                        }));
                    }
                });
            }
            ui.add_space(10.);
            if controls::action(ui, "Create this ordered draft", ButtonKind::Primary).clicked() {
                actions.push(Action::Command(Command::Save(Box::new(DraftInput {
                    id: None,
                    title: proposal.title.clone(),
                    subtitle: proposal.description.clone(),
                    paper_size: PaperSize::Letter,
                    article_ids: proposal.articles.iter().map(|a| a.id.clone()).collect(),
                    refresh_copies: false,
                    design: Design::default(),
                    presentation: Default::default(),
                }))));
            }
        });
        ui.add_space(14.);
    }
    if plans.proposals.is_empty() {
        ui.label("A suggestion needs at least three eligible articles across publications. You can create a manual draft with fewer articles.");
    }
    if !plans.unavailable.is_empty() {
        ui.collapsing(format!("{} unavailable reading copies", plans.unavailable.len()), |ui| {
            ui.label("These publisher copies could not be acquired. They were not silently excluded.");
            for copy in &plans.unavailable {
                ui.label(RichText::new(&copy.title).strong());
                ui.label(RichText::new(&copy.error).size(12.).color(colors.muted));
                ui.add_space(8.);
            }
        });
    }
}
fn review(ui: &mut Ui, state: &mut State, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    let Some(edition) = state.edition.clone() else {
        return;
    };
    let issue = &edition.issue;
    request_cover(ui, state, &issue.id, actions);
    request_previews(ui, state, &issue.id, actions);
    if controls::action(ui, "Back to magazines", ButtonKind::Quiet).clicked() {
        state.edition = None;
        state.plans = None;
        state.editing = None;
        actions.push(Action::Close);
    }
    ui.add_space(12.);
    egui::ScrollArea::vertical()
        .id_salt("magazine-review")
        .show(ui, |ui| {
            if ui.available_width() >= 640. {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(156.);
                        cover(
                            ui,
                            state,
                            &issue.id,
                            &issue.title,
                            issue.issue_number,
                            156.,
                            Some(issue.paper_size),
                        );
                    });
                    ui.add_space(22.);
                    ui.vertical(|ui| {
                        ui.set_max_width(ui.available_width());
                        edition_header(ui, &edition);
                    });
                });
            } else {
                cover(
                    ui,
                    state,
                    &issue.id,
                    &issue.title,
                    issue.issue_number,
                    120.,
                    Some(issue.paper_size),
                );
                ui.add_space(14.);
                edition_header(ui, &edition);
            }
            ui.add_space(22.);
            let busy = edition.production.as_ref().is_some_and(|p| {
                matches!(p.state, ProductionState::Queued | ProductionState::Running)
            });
            ui.horizontal_wrapped(|ui| {
                if edition.artifact.is_some()
                    && controls::action(
                        ui,
                        "Open exact PDF",
                        if issue.finalized_at.is_some() {
                            ButtonKind::Primary
                        } else {
                            ButtonKind::Secondary
                        },
                    )
                    .clicked()
                {
                    state.opened_pdf_sha256 = None;
                    actions.push(Action::OpenPdf(issue.id.clone()));
                }
                if issue.finalized_at.is_none()
                    && ui
                        .add_enabled_ui(!busy, |ui| {
                            controls::action(ui, "Edit edition & design", ButtonKind::Secondary)
                        })
                        .inner
                        .clicked()
                {
                    state.begin_edit();
                }
            });
            ui.add_space(18.);
            if issue.finalized_at.is_none() {
                production_workflow(ui, state, &edition, busy, actions);
            } else {
                controls::badge(ui, "Finalized edition", colors.green);
            }
            ui.add_space(22.);
            ui.label(
                RichText::new("In this edition")
                    .size(22.)
                    .font(egui::FontId::new(
                        22.,
                        egui::FontFamily::Name("Georgia".into()),
                    )),
            );
            ui.add_space(12.);
            for article in &issue.articles {
                controls::panel(ui).inner_margin(16).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(
                            RichText::new(format!("{:02}", article.position + 1))
                                .size(18.)
                                .color(colors.muted),
                        );
                        ui.add_space(8.);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&article.title).size(15.).strong());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(format!("{} words", article.word_count))
                                        .color(colors.muted),
                                );
                                controls::copy_status(ui, article.content_status);
                                ui.hyperlink_to("Open original", &article.url);
                            });
                            if let Some(warning) = &article.content_warning {
                                ui.colored_label(colors.danger, warning);
                            }
                        });
                    });
                });
                ui.add_space(9.);
            }
            ui.add_space(18.);
            edition_details(ui, state, &edition, busy, actions);
        });
    confirmations(ui, state, &edition, actions);
}
fn edition_header(ui: &mut Ui, edition: &Edition) {
    let colors = crate::theme::palette(ui.ctx());
    let issue = &edition.issue;
    ui.horizontal_wrapped(|ui| {
        controls::badge(
            ui,
            &format!("ISSUE {:02}", issue.issue_number),
            colors.green,
        );
        ui.label(RichText::new(crate::domain::date_label(&issue.created_at)).color(colors.muted));
    });
    ui.add_space(10.);
    ui.label(RichText::new(&issue.title).font(egui::FontId::new(
        34.,
        egui::FontFamily::Name("Georgia".into()),
    )));
    if !issue.subtitle.is_empty() {
        ui.label(RichText::new(&issue.subtitle).size(14.).color(colors.muted));
    }
    ui.add_space(10.);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} · {} articles · {} words",
            issue.paper_size.as_str(),
            issue.articles.len(),
            issue
                .articles
                .iter()
                .map(|a| u64::from(a.word_count))
                .sum::<u64>()
        ));
        if let Some(artifact) = &edition.artifact {
            controls::badge(ui, &format!("{} pages", artifact.page_count), colors.green);
        } else if let Some(receipt) = &edition.exported_pdf {
            controls::badge(
                ui,
                &format!("{} pages · exported", receipt.page_count),
                colors.muted,
            );
        }
    });
    if edition.historical {
        ui.label(
            RichText::new(if edition.exported_pdf.is_some() {
                "PDF exported and removed locally. Edition metadata retained."
            } else {
                "Exact viewer PDF retained. Production originals retired."
            })
            .size(12.)
            .color(colors.muted),
        );
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NextStep {
    Content,
    Artwork,
    Upload,
    Pdf,
    Open,
    Review,
    Finalize,
    Working,
}
fn next_step(edition: &Edition, opened: Option<&str>, request: Option<&CoverRequest>) -> NextStep {
    if edition
        .production
        .as_ref()
        .is_some_and(|p| matches!(p.state, ProductionState::Queued | ProductionState::Running))
    {
        return NextStep::Working;
    }
    if let Some(artifact) = &edition.artifact {
        if artifact.reviewed {
            return NextStep::Finalize;
        }
        return if opened == Some(artifact.sha256.as_str()) {
            NextStep::Review
        } else {
            NextStep::Open
        };
    }
    if edition.issue.articles.iter().any(|article| {
        article.content_status == crate::domain::CopyStatus::Pending
            || article.html.trim().is_empty()
    }) {
        return NextStep::Content;
    }
    match request.map(|r| r.state.as_str()) {
        Some("ready") => NextStep::Pdf,
        Some("waiting") => NextStep::Upload,
        _ => NextStep::Artwork,
    }
}
fn production_workflow(
    ui: &mut Ui,
    state: &mut State,
    edition: &Edition,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    let issue = &edition.issue;
    let request = state
        .cover_request
        .as_ref()
        .or(edition.cover_request.as_ref())
        .cloned();
    let next = next_step(
        edition,
        state.opened_pdf_sha256.as_deref(),
        request.as_ref(),
    );
    let current = match next {
        NextStep::Content => 0,
        NextStep::Artwork | NextStep::Upload => 2,
        NextStep::Pdf | NextStep::Working => 3,
        _ => 4,
    };
    controls::panel(ui).show(ui,|ui|{
        ui.label(RichText::new("Bring the edition to paper").size(17.).strong());
        ui.horizontal_wrapped(|ui|{for(index,label)in ["Content","Design","Artwork","PDF","Review"].into_iter().enumerate(){controls::badge(ui,&format!("{} · {label}{}",index+1,if index==current{" · Current"}else if index<current{" · Done"}else{""}),if index<=current{colors.green}else{colors.muted});}});
        ui.add_space(12.);
        if let Some(production)=&edition.production{
            ui.label(RichText::new(&production.stage).color(colors.green));
            if busy{controls::status(ui,if production.state==ProductionState::Queued{controls::Status::Queued}else{controls::Status::Loading});ui.add(egui::ProgressBar::new(production.progress as f32/100.).text(format!("{}%",production.progress)));}
            else if production.state==ProductionState::Done{controls::status(ui,controls::Status::Completed);}
            if let Some(error)=&production.error{controls::status(ui,controls::Status::Failed);ui.colored_label(colors.danger,error);}
        }
        ui.checkbox(&mut state.acknowledged,"I acknowledge any partial or failed reading copies in this edition");
        ui.add_space(10.);
        ui.horizontal_wrapped(|ui|{
            let label=match next{NextStep::Content=>"Resolve reading copies",NextStep::Artwork=>"Request cover paintings",NextStep::Upload=>"Add front & back originals…",NextStep::Pdf=>if edition.production.as_ref().is_some_and(|p|p.state==ProductionState::Failed){"Retry PDF"}else{"Generate PDF"},NextStep::Open=>"Open exact PDF",NextStep::Review=>"I reviewed this exact PDF",NextStep::Finalize=>"Finalize edition",NextStep::Working=>"Producing PDF…"};
            if ui.add_enabled_ui(!busy,|ui|controls::action(ui,label,ButtonKind::Primary)).inner.clicked(){match next{
                NextStep::Content=>state.begin_edit(),
                NextStep::Artwork=>actions.push(Action::Command(Command::RequestCovers(issue.id.clone()))),
                NextStep::Upload=>{if let Some(request)=&request{actions.push(Action::PickCovers{magazine_id:issue.id.clone(),request_id:request.id.clone(),input_sha256:request.input_sha256.clone()});}},
                NextStep::Pdf=>actions.push(Action::Command(Command::Queue{id:issue.id.clone(),acknowledged:state.acknowledged})),
                NextStep::Open=>{state.opened_pdf_sha256=None;actions.push(Action::OpenPdf(issue.id.clone()));},
                NextStep::Review=>{if let Some(artifact)=&edition.artifact{actions.push(Action::Command(Command::Reviewed{id:issue.id.clone(),sha256:artifact.sha256.clone()}));}},
                NextStep::Finalize=>actions.push(Action::Command(Command::Finalize(issue.id.clone()))),
                NextStep::Working=>{},
            }}
            ui.menu_button("Edition actions",|ui|{
                if ui.add_enabled_ui(!busy,|ui|controls::action(ui,"Generate / retry PDF",ButtonKind::Quiet)).inner.clicked(){actions.push(Action::Command(Command::Queue{id:issue.id.clone(),acknowledged:state.acknowledged}));ui.close();}
                if ui.add_enabled_ui(!busy,|ui|controls::action(ui,"Request cover paintings",ButtonKind::Quiet)).inner.clicked(){actions.push(Action::Command(Command::RequestCovers(issue.id.clone())));ui.close();}
                if let Some(artifact)=&edition.artifact
                    && ui.add_enabled_ui(artifact.reviewed&&!busy,|ui|controls::action(ui,"Finalize edition",ButtonKind::Quiet)).inner.clicked(){actions.push(Action::Command(Command::Finalize(issue.id.clone())));ui.close();
                }
            });
        });
        ui.label(RichText::new(match next{NextStep::Content=>"Empty or pending copies must be resolved before production.",NextStep::Artwork=>"Create a paired cover request from the saved content and design.",NextStep::Upload=>"Keep the paintings text-free. Add both original files after generating them.",NextStep::Pdf=>"The renderer uses frozen article copies and verified artwork, with network access disabled.",NextStep::Open=>"Open the stored PDF and inspect every page before acknowledging this exact copy.",NextStep::Review=>"Your acknowledgement is bound to the checksum of the PDF you opened.",NextStep::Finalize=>"The current PDF is reviewed. Finalization freezes its bytes and retires draft production originals.",NextStep::Working=>"Production is running. You can read the contents below while it finishes."}).size(12.).color(colors.muted));
    });
    if let Some(request) = state
        .cover_request
        .as_ref()
        .or(edition.cover_request.as_ref())
    {
        ui.add_space(16.);
        artwork(ui, state, request, &issue.id, actions);
    }
}
fn artwork(
    ui: &mut Ui,
    state: &State,
    request: &CoverRequest,
    _id: &MagazineId,
    actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    controls::panel(ui).show(ui,|ui|{
        ui.horizontal_wrapped(|ui|{ui.label(RichText::new("Paired cover paintings").size(17.).strong());controls::status(ui,if request.state=="ready"{controls::Status::Completed}else{controls::Status::Waiting});});
        ui.label(RichText::new("Generate the front with Codex. Inspect it, then use it as the reference for the back. Keep both paintings text-free.").color(colors.muted));
        ui.add_space(12.);
        let cards=[("Front painting","Create the visual direction","frontPrompt","cover:front",request.front_asset.is_some()),("Back painting","Continue the front's composition","backPrompt","cover:back",request.back_asset.is_some())];
        let draw_card=|ui:&mut Ui,(title,detail,prompt,key,uploaded):(&str,&str,&str,&str,bool)|{
            controls::panel(ui).inner_margin(14).show(ui,|ui|{
                ui.label(RichText::new(title).strong());ui.label(RichText::new(detail).size(11.).color(colors.muted));
                if uploaded{controls::badge(ui,"Original uploaded",colors.green);}else{controls::status(ui,controls::Status::Waiting);}
                asset_well(ui,state,key,title,190.);
                if controls::action(ui,"Copy prompt",ButtonKind::Secondary).clicked(){ui.ctx().copy_text(request.request[prompt].as_str().unwrap_or_default().into());}
            });
        };
        if ui.available_width()>=500.{ui.columns(2,|columns|{for(index,card)in cards.into_iter().enumerate(){draw_card(&mut columns[index],card);}});}else{for card in cards{draw_card(ui,card);ui.add_space(10.);}}
        if controls::action(ui,"Export cover request",ButtonKind::Quiet).clicked(){actions.push(Action::ExportRequest(request.clone()));}
        ui.label(RichText::new("Previews are transient projections of retained print derivatives. Original files and exact hashes stay with this editable draft until finalization.").size(12.).color(colors.muted));
    });
}
fn edition_details(
    ui: &mut Ui,
    state: &mut State,
    edition: &Edition,
    busy: bool,
    _actions: &mut Vec<Action>,
) {
    let issue = &edition.issue;
    ui.collapsing("Edition details & storage", |ui| {
        if issue.finalized_at.is_some() {
            ui.label("Finalized edition metadata and PDF checksums are frozen.");
        }
        if edition.historical {
            ui.label("Viewer-only edition · production originals retired");
        }
        if let Some(artifact) = &edition.artifact {
            ui.label(format!("PDF SHA-256: {}", artifact.sha256));
        }
        if let Some(production) = &edition.production {
            ui.label(format!(
                "Production attempt {} · {}% · {}",
                production.attempts, production.progress, production.stage
            ));
        }
        if let Some(receipt) = &edition.exported_pdf {
            ui.label(format!("Exported {}", receipt.exported_at));
            ui.label(&receipt.path);
            ui.label(format!("PDF SHA-256: {}", receipt.sha256));
        }
        if let Some(provenance) = &state.provenance {
            ui.collapsing("Retained production provenance", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(220.)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(provenance).monospace().size(11.))
                                .wrap(),
                        );
                    });
            });
        }
        if edition.can_export_remove_pdf
            && let Some(artifact) = &edition.artifact
            && controls::action(
                ui,
                "Export PDF and free local space…",
                ButtonKind::Secondary,
            )
            .clicked()
        {
            state.remove_pdf_confirm = Some((issue.id.clone(), artifact.sha256.clone()));
        }
        if issue.finalized_at.is_none()
            && ui
                .add_enabled_ui(!busy, |ui| {
                    controls::action(ui, "Delete draft…", ButtonKind::Quiet)
                })
                .inner
                .clicked()
        {
            state.delete_confirm = true;
            state.delete_acknowledged = false;
        }
    });
}
fn confirmations(ui: &mut Ui, state: &mut State, edition: &Edition, actions: &mut Vec<Action>) {
    let busy = edition
        .production
        .as_ref()
        .is_some_and(|p| matches!(p.state, ProductionState::Queued | ProductionState::Running));
    if state.delete_confirm && edition.issue.finalized_at.is_none() {
        let mut close = false;
        let modal=egui::Modal::new(egui::Id::new("delete-edition-confirmation")).frame(controls::panel(ui)).show(ui.ctx(),|ui|{
            ui.set_width((ui.ctx().content_rect().width()-96.).clamp(240.,460.));
            ui.label(RichText::new("Delete this draft?").size(22.).strong());ui.label(RichText::new(&edition.issue.title).size(16.));
            ui.label(format!("Remove this editable draft and its {} frozen article copies, artwork and production files. Your reading-list articles keep their current state.",edition.issue.articles.len()));
            ui.checkbox(&mut state.delete_acknowledged,"Confirm removing this editable draft");
            ui.horizontal_wrapped(|ui|{
                if controls::action(ui,"Cancel",ButtonKind::Secondary).clicked(){close=true;}
                if ui.add_enabled_ui(state.delete_acknowledged&&!busy,|ui|controls::action(ui,"Delete draft",ButtonKind::Danger)).inner.clicked(){actions.push(Action::Command(Command::Delete(edition.issue.id.clone())));close=true;}
            });
        });
        if close || modal.should_close() {
            state.delete_confirm = false;
            state.delete_acknowledged = false;
        }
    }
    if let Some((id, sha)) = state.remove_pdf_confirm.clone() {
        let eligible = edition.can_export_remove_pdf
            && edition.issue.id == id
            && edition
                .artifact
                .as_ref()
                .is_some_and(|artifact| artifact.sha256 == sha);
        if !eligible {
            state.remove_pdf_confirm = None;
            return;
        }
        let mut close = false;
        let modal=egui::Modal::new(egui::Id::new("remove-edition-pdf-confirmation")).frame(controls::panel(ui)).show(ui.ctx(),|ui|{
            ui.set_width((ui.ctx().content_rect().width()-96.).clamp(240.,500.));
            ui.label(RichText::new("Export before freeing local space").size(21.).strong());ui.label(RichText::new(&edition.issue.title).size(16.));
            ui.label("Choose a destination outside the library. The exact PDF is verified before its local copy is removed. Keep the exported file: this app will retain edition metadata, checksums, provenance and printed-article references.");
            ui.collapsing("Exact PDF checksum",|ui|{ui.label(&sha);});
            ui.horizontal_wrapped(|ui|{
                if controls::action(ui,"Cancel",ButtonKind::Secondary).clicked(){close=true;}
                if controls::action(ui,"Choose export location & remove local PDF",ButtonKind::Danger).clicked(){actions.push(Action::ChoosePdfRemoval{id:id.clone(),sha256:sha.clone(),title:edition.issue.title.clone()});close=true;}
            });
        });
        if close || modal.should_close() {
            state.remove_pdf_confirm = None;
        }
    }
}
fn font(ui: &mut Ui, id: &str, label: &str, value: &mut Font) {
    let colors = crate::theme::palette(ui.ctx());
    ui.label(RichText::new(label).size(12.).color(colors.muted));
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .show_ui(ui, |ui| {
            for candidate in [
                Font::Georgia,
                Font::Baskerville,
                Font::Avenir,
                Font::Helvetica,
            ] {
                ui.selectable_value(value, candidate, candidate.as_str());
            }
        });
}
fn draft_signature(input: &DraftInput) -> serde_json::Value {
    serde_json::json!({"id":input.id,"title":input.title,"subtitle":input.subtitle,"paper":input.paper_size,"articles":input.article_ids,"refresh":input.refresh_copies,"design":input.design,"presentation":input.presentation})
}
fn editor(ui: &mut Ui, state: &mut State, actions: &mut Vec<Action>) {
    let colors = crate::theme::palette(ui.ctx());
    let Some(mut input) = state.editing.take() else {
        return;
    };
    let issue = state.edition.as_ref().map(|e| e.issue.clone());
    if let Some(id) = &input.id {
        request_previews(ui, state, id, actions);
    }
    ui.horizontal_wrapped(|ui| {
        if controls::action(ui, "Back to magazines", ButtonKind::Quiet).clicked() {
            actions.push(Action::Close);
        }
        ui.label(RichText::new("Edition studio").font(egui::FontId::new(
            27.,
            egui::FontFamily::Name("Georgia".into()),
        )));
        controls::status(ui, controls::Status::Draft);
    });
    ui.add_space(12.);
    let mut save = false;
    let dirty =
        input.id.is_none() || state.edit_baseline.as_ref() != Some(&draft_signature(&input));
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled_ui(state.can_save(&input), |ui| {
                controls::action(ui, "Save ordered edition", ButtonKind::Primary)
            })
            .inner
            .clicked()
        {
            save = true;
        }
        controls::badge(
            ui,
            if input.id.is_none() {
                "New draft · not saved"
            } else if dirty {
                "Unsaved changes"
            } else {
                "All changes saved"
            },
            if dirty { colors.green } else { colors.muted },
        );
        ui.label(
            RichText::new(format!("{} whole articles", input.article_ids.len()))
                .size(12.)
                .color(colors.muted),
        );
    });
    let pending = state.relevant_pending_repairs(&input);
    if pending > 0 {
        ui.horizontal_wrapped(|ui|{controls::status(ui,controls::Status::Waiting);ui.label(format!("Retaining {pending} source original(s). Save will be available when the work finishes."));});
    }
    if let Some(error) = state.repair_error.clone() {
        controls::panel(ui).inner_margin(14).show(ui,|ui|{
            controls::status(ui,controls::Status::Failed);
            ui.label("A source original could not be retained. Open Corrections & images to retry; your draft changes are still here.");
            ui.collapsing("Repair error details",|ui|{ui.label(&error.detail);});
            if controls::action(ui,"Dismiss repair error",ButtonKind::Quiet).clicked(){state.repair_error=None;}
        });
    }
    ui.add_space(14.);
    egui::ScrollArea::vertical()
        .id_salt("magazine-editor")
        .show(ui, |ui| {
            controls::panel(ui).show(ui, |ui| {
                ui.label(RichText::new("The edition").size(17.).strong());
                ui.label("Title");
                controls::field(ui, &mut input.title, "Give this edition a title");
                ui.label("Subtitle / editorial theme");
                controls::field(
                    ui,
                    &mut input.subtitle,
                    "What brings these articles together?",
                );
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("edition-paper")
                        .selected_text(input.paper_size.as_str())
                        .show_ui(ui, |ui| {
                            for paper in [PaperSize::Letter, PaperSize::A4, PaperSize::A5] {
                                ui.selectable_value(&mut input.paper_size, paper, paper.as_str());
                            }
                        });
                    egui::ComboBox::from_id_salt("edition-profile")
                        .selected_text(if input.design.profile == PrintProfile::Home {
                            "Home / print PDF"
                        } else {
                            "Canva magazine"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut input.design.profile,
                                PrintProfile::Home,
                                "Home / print PDF",
                            );
                            ui.selectable_value(
                                &mut input.design.profile,
                                PrintProfile::Canva,
                                "Canva: Letter, 2 columns, 24–204 even pages",
                            );
                        });
                });
            });
            ui.add_space(16.);
            if ui.available_width() >= 1000. {
                ui.columns(2, |columns| {
                    contents_editor(&mut columns[0], state, &mut input, issue.as_ref(), actions);
                    theme_studio(&mut columns[1], &mut input);
                });
            } else {
                contents_editor(ui, state, &mut input, issue.as_ref(), actions);
                ui.add_space(16.);
                theme_studio(ui, &mut input);
            }
        });
    if let Some(article_id) = state.repair_open.clone() {
        if let Some(article) = issue.as_ref().and_then(|issue| {
            issue
                .articles
                .iter()
                .find(|article| article.article_id == article_id)
        }) {
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::new("edition-source-repairs"))
                .frame(controls::panel(ui))
                .show(ui.ctx(), |ui| {
                    ui.set_width((ui.ctx().content_rect().width() - 96.).clamp(240., 650.));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Corrections & source repairs")
                                .size(20.)
                                .strong(),
                        );
                        if controls::icon_button(ui, Icon::Close, "Close corrections", false)
                            .clicked()
                        {
                            close = true;
                        }
                    });
                    ui.label(RichText::new(&article.title).size(14.).strong());
                    egui::ScrollArea::vertical()
                        .id_salt("repair-form")
                        .max_height((ui.ctx().content_rect().height() - 230.).max(120.))
                        .show(ui, |ui| {
                            repairs(
                                ui,
                                state,
                                &mut input.presentation,
                                input.id.as_ref(),
                                input
                                    .article_ids
                                    .iter()
                                    .position(|id| id == &article_id)
                                    .unwrap_or(0),
                                article,
                                actions,
                            );
                        });
                    if controls::action(ui, "Done", ButtonKind::Primary).clicked() {
                        close = true;
                    }
                });
            if close || modal.should_close() {
                state.repair_open = None;
            }
        } else {
            state.repair_open = None;
        }
    }
    // Emit only after every field, preset and repair edit has been applied.
    let choosing_repair=actions.iter().any(|action|matches!(action,Action::PickRepair(repair) if repair.context.as_ref().is_some_and(|context|context.edit_generation==state.edit_generation&&input.id.as_ref()==Some(&context.magazine_id))));
    if save && state.can_save(&input) && !choosing_repair {
        actions.push(Action::Command(Command::Save(Box::new(input.clone()))));
        state.loading = true;
    }
    state.editing = Some(input);
}
fn contents_editor(
    ui: &mut Ui,
    state: &mut State,
    input: &mut DraftInput,
    issue: Option<&Issue>,
    _actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    controls::panel(ui).show(ui,|ui|{
        ui.label(RichText::new("Whole articles, in print order").size(17.).strong());
        ui.label(RichText::new("Drag the handle to reorder. The arrow controls also work with Tab and Space.").size(12.).color(colors.muted));
        let mut move_up=None;let mut move_down=None;let mut remove=None;let mut dropped=None;
        for (index,id) in input.article_ids.iter().enumerate(){
            let article=issue.and_then(|issue|issue.articles.iter().find(|a|&a.article_id==id));
            let metadata=state.manual_articles.get(id);
            let title=article.map(|a|a.title.as_str()).or_else(||metadata.map(|a|a.title.as_str())).unwrap_or("Selected article");
            let url=article.map(|a|a.url.as_str()).or_else(||metadata.map(|a|a.url.as_str())).unwrap_or("");
            let publication=metadata.map(|a|a.site.clone()).filter(|site|!site.is_empty()).unwrap_or_else(||url::Url::parse(url).ok().and_then(|url|url.host_str().map(|host|host.trim_start_matches("www.").to_owned())).unwrap_or_else(||"Publication unavailable".into()));
            let words=article.map(|a|a.word_count).or_else(||metadata.map(|a|a.word_count)).unwrap_or(0);
            let (_,payload)=ui.dnd_drop_zone::<ArticleId,_>(egui::Frame::new().inner_margin(10).corner_radius(8),|ui|{
                ui.horizontal_top(|ui|{
                    ui.dnd_drag_source(ui.id().with(("article-drag",&id.0)),id.clone(),|ui|{
                        let (rect,response)=ui.allocate_exact_size(egui::vec2(24.,32.),egui::Sense::hover());super::icons::draw(ui,rect.shrink2(egui::vec2(4.,7.)),Icon::Grip,colors.muted);response.on_hover_text("Drag to change print order");
                    });
                    ui.label(RichText::new(format!("{:02}",index+1)).size(17.).color(colors.muted));
                    ui.vertical(|ui|{
                        ui.label(RichText::new(title).size(14.).strong());
                        ui.label(RichText::new(format!("{publication} · {}",if words==0{"Reading time unknown".into()}else{format!("{} min · {words} words",words.div_ceil(230))})).size(11.).color(colors.muted));
                    });
                });
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled_ui(index>0,|ui|controls::icon_button(ui,Icon::ChevronUp,"Move article earlier",false)).inner.clicked(){move_up=Some(index);}
                    if ui.add_enabled_ui(index+1<input.article_ids.len(),|ui|controls::icon_button(ui,Icon::ChevronDown,"Move article later",false)).inner.clicked(){move_down=Some(index);}
                    if controls::icon_button(ui,Icon::Trash,"Remove article from draft",false).clicked(){remove=Some(index);}
                    if article.is_some(){if controls::action(ui,"Corrections & images",ButtonKind::Quiet).clicked(){state.repair_article=index;state.repair_open=Some(id.clone());}}
                    else{ui.label(RichText::new("Save once to edit source corrections").size(11.).color(colors.muted));}
                });
            });
            if let Some(payload)=payload{dropped=Some(((*payload).clone(),index));}
            ui.separator();
        }
        if !input.article_ids.is_empty(){
            let (_,payload)=ui.dnd_drop_zone::<ArticleId,_>(egui::Frame::new().inner_margin(10).corner_radius(8),|ui|{ui.label(RichText::new("Drop here to place an article last").size(11.).color(colors.muted));});
            if let Some(payload)=payload{dropped=Some(((*payload).clone(),input.article_ids.len()));}
        }
        if let Some((id,to))=dropped{move_article(&mut input.article_ids,&id,to);}
        if let Some(index)=move_up{input.article_ids.swap(index,index-1);}
        if let Some(index)=move_down{input.article_ids.swap(index,index+1);}
        if let Some(index)=remove{let id=input.article_ids.remove(index);input.presentation.remove(&id);}
        ui.add_space(16.);ui.checkbox(&mut input.refresh_copies,"Explicitly fetch and refresh frozen article snapshots");
        ui.label(RichText::new("Saving changes or refreshing copies invalidates the previous production proof. Original snapshots remain separate from corrections and source repairs.").color(colors.muted).size(12.));
    });
}
fn move_article(ids: &mut Vec<ArticleId>, id: &ArticleId, before: usize) {
    if let Some(from) = ids.iter().position(|candidate| candidate == id) {
        let value = ids.remove(from);
        let target = before
            .saturating_sub(usize::from(from < before))
            .min(ids.len());
        ids.insert(target, value);
    }
}
fn theme_studio(ui: &mut Ui, input: &mut DraftInput) {
    let colors = crate::theme::palette(ui.ctx());
    controls::panel(ui).show(ui, |ui| {
        ui.label(RichText::new("Theme studio").size(17.).strong());
        ui.label(RichText::new("Choose a direction, then tune the page.").color(colors.muted));
        ui.add_space(12.);
        let presets = themes();
        let columns = if ui.available_width() >= 640. {
            4
        } else if ui.available_width() >= 300. {
            2
        } else {
            1
        };
        let card_width =
            ((ui.available_width() - 12. * (columns - 1) as f32) / columns as f32).max(90.);
        egui::Grid::new("theme-cards")
            .num_columns(columns)
            .spacing(egui::vec2(12., 12.))
            .show(ui, |ui| {
                for (index, theme) in presets.into_iter().enumerate() {
                    let selected = input.design.theme.name == theme.name;
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(card_width, 104.), egui::Sense::click());
                    let paper = color(
                        &theme.paper,
                        crate::theme::Appearance::Light.palette().reader,
                    );
                    let ink = color(&theme.ink, crate::theme::Appearance::Light.palette().ink);
                    ui.painter().rect_filled(rect, 8, paper);
                    ui.painter().rect_stroke(
                        rect,
                        8,
                        egui::Stroke::new(
                            if selected { 2. } else { 1. },
                            if selected {
                                colors.green
                            } else {
                                colors.divider
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.min + egui::vec2(13., 10.),
                        egui::Align2::LEFT_TOP,
                        "Aa",
                        preview_font(ui, theme.display_font, 31.),
                        ink,
                    );
                    ui.painter().line_segment(
                        [
                            rect.min + egui::vec2(14., 58.),
                            rect.min + egui::vec2(card_width - 14., 58.),
                        ],
                        egui::Stroke::new(
                            2.,
                            color(
                                &theme.accent,
                                crate::theme::Appearance::Light.palette().green,
                            ),
                        ),
                    );
                    ui.painter().text(
                        rect.min + egui::vec2(13., 72.),
                        egui::Align2::LEFT_TOP,
                        &theme.name,
                        egui::FontId::proportional(11.),
                        ink,
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::RadioButton,
                            true,
                            format!(
                                "{} theme{}",
                                theme.name,
                                if selected { ", selected" } else { "" }
                            ),
                        )
                    });
                    controls::focus(ui, &response);
                    if response.clicked() {
                        input.design.theme = theme;
                    }
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(18.);
        if ui.available_width() >= 760. {
            ui.columns(2, |columns| {
                design_controls(&mut columns[0], input);
                page_preview(&mut columns[1], input);
            });
        } else {
            design_controls(ui, input);
            ui.add_space(16.);
            page_preview(ui, input);
        }
    });
}
fn design_controls(ui: &mut Ui, input: &mut DraftInput) {
    let colors = crate::theme::palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        font(
            ui,
            "display-font",
            "Display font",
            &mut input.design.theme.display_font,
        );
        font(
            ui,
            "body-font",
            "Body font",
            &mut input.design.theme.body_font,
        );
    });
    ui.add_space(10.);
    controls::slider(
        ui,
        egui::Slider::new(&mut input.design.body_size_pt, 8.0..=14.0).text("Body size · pt"),
    );
    controls::slider(
        ui,
        egui::Slider::new(&mut input.design.line_height, 1.15..=1.8).text("Line height"),
    );
    controls::slider(
        ui,
        egui::Slider::new(&mut input.design.column_gap_pt, 16.0..=36.0).text("Column gap · pt"),
    );
    controls::slider(
        ui,
        egui::Slider::new(&mut input.design.paragraph_space_pt, 3.0..=14.0)
            .text("Paragraph spacing · pt"),
    );
    egui::ComboBox::from_id_salt("edition-columns")
        .selected_text(columns_label(input.design.columns))
        .show_ui(ui, |ui| {
            for columns in [Columns::Auto, Columns::One, Columns::Two] {
                ui.selectable_value(&mut input.design.columns, columns, columns_label(columns));
            }
        });
    ui.add_space(10.);
    ui.collapsing("Palette & cover prompt versions", |ui| {
        for (label, value) in [
            ("Paper", &mut input.design.theme.paper),
            ("Ink", &mut input.design.theme.ink),
            ("Accent", &mut input.design.theme.accent),
            ("Muted", &mut input.design.theme.muted),
        ] {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 16.), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 4, color(value, colors.muted));
                ui.label(label);
            });
            controls::field(ui, value, "#rrggbb");
        }
        ui.label("Front painting prompt");
        prompt_picker(
            ui,
            "front-version",
            &mut input.design.front_prompt,
            &[
                ("japanese-watercolor-v1", "Watercolor · first study"),
                ("japanese-watercolor-v2", "Watercolor · second study"),
                ("japanese-watercolor-v3", "Watercolor · third study"),
                ("japanese-watercolor-v4", "Watercolor · fourth study"),
                ("japanese-watercolor-v5", "Watercolor · current front"),
            ],
        );
        ui.label("Back painting prompt");
        prompt_picker(
            ui,
            "back-version",
            &mut input.design.back_prompt,
            &[
                (
                    "japanese-watercolor-back-v1",
                    "Watercolor · first continuation",
                ),
                (
                    "japanese-watercolor-back-v2",
                    "Watercolor · current continuation",
                ),
            ],
        );
    });
}
fn columns_label(columns: Columns) -> &'static str {
    match columns {
        Columns::Auto => "Columns: automatic",
        Columns::One => "Columns: one",
        Columns::Two => "Columns: two",
    }
}
fn prompt_picker(ui: &mut Ui, id: &str, value: &mut String, versions: &[(&str, &str)]) {
    let label = versions
        .iter()
        .find(|(key, _)| *key == value.as_str())
        .map(|(_, label)| *label)
        .unwrap_or(value.as_str())
        .to_owned();
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .show_ui(ui, |ui| {
            for (key, label) in versions {
                ui.selectable_value(value, (*key).into(), format!("{label} · {key}"));
            }
        });
}
fn color(value: &str, fallback: egui::Color32) -> egui::Color32 {
    let Some(hex) = value.strip_prefix('#').filter(|hex| hex.len() == 6) else {
        return fallback;
    };
    u32::from_str_radix(hex, 16)
        .map(|rgb| egui::Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        .unwrap_or(fallback)
}
fn preview_font(ui: &Ui, font: Font, size: f32) -> egui::FontId {
    let name = match font {
        Font::Georgia => "Georgia",
        Font::Baskerville => "Baskerville",
        Font::Avenir => "Avenir Next",
        Font::Helvetica => "Helvetica Neue",
    };
    let family = egui::FontFamily::Name(name.into());
    if ui
        .ctx()
        .fonts(|fonts| fonts.definitions().families.contains_key(&family))
    {
        egui::FontId::new(size, family)
    } else {
        egui::FontId::proportional(size)
    }
}
fn page_preview(ui: &mut Ui, input: &DraftInput) {
    let colors = crate::theme::palette(ui.ctx());
    ui.label(RichText::new("Representative page").strong());
    ui.label(RichText::new("Typography study only. Generate and review the exact PDF to check pagination, content, and print geometry.").size(11.).color(colors.muted));
    let width = ui.available_width().min(310.);
    let (paper_w, paper_h) = input.paper_size.points();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, width * (paper_h / paper_w) as f32),
        egui::Sense::hover(),
    );
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(
        rect,
        3,
        color(
            &input.design.theme.paper,
            crate::theme::Appearance::Light.palette().reader,
        ),
    );
    painter.rect_stroke(
        rect,
        3,
        egui::Stroke::new(1., colors.divider),
        egui::StrokeKind::Inside,
    );
    let margin = width * 0.085;
    let left = rect.left() + margin;
    let top = rect.top() + margin;
    let ink = color(
        &input.design.theme.ink,
        crate::theme::Appearance::Light.palette().ink,
    );
    let accent = color(
        &input.design.theme.accent,
        crate::theme::Appearance::Light.palette().green,
    );
    painter.text(
        egui::pos2(left, top),
        egui::Align2::LEFT_TOP,
        "THE READING LIST",
        egui::FontId::proportional(7.),
        color(
            &input.design.theme.muted,
            crate::theme::Appearance::Light.palette().muted,
        ),
    );
    let title = if input.title.trim().is_empty() {
        "A longer read"
    } else {
        &input.title
    };
    let title_galley = painter.layout(
        title.into(),
        preview_font(ui, input.design.theme.display_font, 21.),
        ink,
        width - margin * 2.,
    );
    let body_top = top + 22. + title_galley.size().y + 16.;
    painter.galley(egui::pos2(left, top + 22.), title_galley, ink);
    painter.line_segment(
        [
            egui::pos2(left, body_top - 8.),
            egui::pos2(rect.right() - margin, body_top - 8.),
        ],
        egui::Stroke::new(1.5, accent),
    );
    let count = input.design.column_count(input.paper_size);
    let scale = width / paper_w as f32;
    let gap = input.design.column_gap_pt as f32 * scale;
    let column_width = (width - margin * 2. - gap * (count - 1) as f32) / count as f32;
    let prose = "A quiet moment belongs to a good story. The page gives each idea room to unfold, with words gathered into a rhythm of their own. Reading asks us to slow down and notice what stays with us.";
    for column in 0..count {
        let x = left + column as f32 * (column_width + gap);
        let mut y = body_top;
        let size = (input.design.body_size_pt as f32 * scale).max(5.);
        let mut job = egui::text::LayoutJob::simple(
            prose.into(),
            preview_font(ui, input.design.theme.body_font, size),
            ink,
            column_width,
        );
        job.sections[0].format.line_height = Some(size * input.design.line_height as f32);
        let galley = painter.layout_job(job);
        while y + galley.size().y < rect.bottom() - margin {
            painter.galley(egui::pos2(x, y), galley.clone(), ink);
            y += galley.size().y + input.design.paragraph_space_pt as f32 * scale;
        }
    }
}
fn repairs(
    ui: &mut Ui,
    state: &mut State,
    presentations: &mut std::collections::HashMap<ArticleId, Presentation>,
    magazine_id: Option<&MagazineId>,
    index: usize,
    article: &Snapshot,
    actions: &mut Vec<Action>,
) {
    let colors = crate::theme::palette(ui.ctx());
    ui.vertical(|ui| {
        let presentation = presentations.entry(article.article_id.clone()).or_default();
        ui.label(
            RichText::new(
                "Presentation corrections preserve the original snapshot and its evidence.",
            )
            .size(12.)
            .color(colors.muted),
        );
        let author = presentation
            .display_author
            .get_or_insert_with(|| article.author.clone().unwrap_or_default());
        ui.label("Display author");
        controls::field(ui, author, "Author as it should appear in print");
        let source = presentation.author_source.get_or_insert_default();
        ui.label("Original author source URL");
        controls::field(ui, source, "https://…");
        if source.trim().is_empty() && Some(author.as_str()) == article.author.as_deref() {
            presentation.display_author = None;
            presentation.author_source = None;
        }
        let date = presentation.published_date.get_or_insert_default();
        ui.label("Corrected publication date");
        controls::field(ui, &mut date.display, "Publication date");
        ui.label("Date source URL");
        controls::field(ui, &mut date.source_url, "https://…");
        if date.display.trim().is_empty() && date.source_url.trim().is_empty() {
            presentation.published_date = None;
        }
        for image in crate::rich_content::RichDocument::parse(&article.html).image_sources() {
            asset_well(
                ui,
                state,
                &format!("source:{image}"),
                "Retained source image",
                150.,
            );
            ui.label(RichText::new(image).size(11.).color(colors.muted));
            let layout = presentation
                .image_layouts
                .entry(image.into())
                .or_insert_with(|| "illustration".into());
            egui::ComboBox::from_id_salt(("image-layout", article.article_id.0.clone(), image))
                .selected_text(layout.as_str())
                .show_ui(ui, |ui| {
                    ui.selectable_value(layout, "illustration".into(), "Illustration");
                    ui.selectable_value(layout, "data".into(), "Data / preserve detail");
                });
        }
        for repair in &presentation.insertions {
            asset_well(
                ui,
                state,
                &format!("asset:{}", repair.asset_id),
                &repair.alt,
                150.,
            );
            ui.label(format!(
                "Recovered image: {} · {}",
                repair.alt, repair.sha256
            ));
        }
        if controls::action(
            ui,
            "Recover an image after a source paragraph",
            ButtonKind::Secondary,
        )
        .clicked()
        {
            state.repair_article = index;
            state.repair_source = article.url.clone();
            state.repair_anchor.clear();
        }
        if state.repair_article == index {
            let document = scraper::Html::parse_fragment(&article.html);
            let selector = scraper::Selector::parse("p").expect("constant paragraph selector");
            let paragraphs: Vec<_> = document
                .select(&selector)
                .map(|p| p.text().collect::<String>().trim().to_owned())
                .collect();
            egui::ComboBox::from_id_salt(("repair-anchor", article.article_id.0.clone()))
                .width(ui.available_width().min(400.))
                .selected_text(if state.repair_anchor.is_empty() {
                    "Select exact unique paragraph".to_owned()
                } else {
                    paragraph_excerpt(&state.repair_anchor)
                })
                .show_ui(ui, |ui| {
                    for paragraph in &paragraphs {
                        if paragraphs.iter().filter(|p| *p == paragraph).count() == 1 {
                            ui.selectable_value(
                                &mut state.repair_anchor,
                                paragraph.clone(),
                                paragraph_excerpt(paragraph),
                            )
                            .on_hover_text(paragraph);
                        }
                    }
                });
            if !state.repair_anchor.is_empty() {
                controls::panel(ui).inner_margin(12).show(ui, |ui| {
                    ui.label(
                        RichText::new("Image will follow this exact paragraph")
                            .size(11.)
                            .color(colors.muted),
                    );
                    egui::ScrollArea::vertical()
                        .max_height(120.)
                        .show(ui, |ui| {
                            ui.label(&state.repair_anchor);
                        });
                });
            }
            for (label, value) in [
                ("Image alt text", &mut state.repair_alt),
                ("Source caption / note", &mut state.repair_note),
                ("Source page URL", &mut state.repair_source),
                ("Original image URL", &mut state.repair_image_url),
                ("Original file label", &mut state.repair_label),
            ] {
                ui.label(label);
                controls::field(ui, value, label);
            }
            if let Some(magazine_id) = magazine_id {
                if ui
                    .add_enabled_ui(
                        !state.repair_anchor.is_empty()
                            && !state.repair_alt.trim().is_empty()
                            && !state.repair_note.trim().is_empty(),
                        |ui| {
                            controls::action(
                                ui,
                                "Retain source image original…",
                                ButtonKind::Secondary,
                            )
                        },
                    )
                    .inner
                    .clicked()
                {
                    actions.push(Action::PickRepair(RepairInput {
                        context: Some(Box::new(RepairContext {
                            request_id: uuid::Uuid::new_v4(),
                            edit_generation: state.edit_generation,
                            magazine_id: magazine_id.clone(),
                            article_id: article.article_id.clone(),
                            after_text: state.repair_anchor.clone(),
                            alt: state.repair_alt.clone(),
                            note: state.repair_note.clone(),
                        })),
                        magazine_id: magazine_id.clone(),
                        body: Vec::new(),
                        label: state.repair_label.clone(),
                        source_url: state.repair_source.clone(),
                        image_url: state.repair_image_url.clone(),
                        retrieved_at: chrono::Utc::now().to_rfc3339(),
                    }));
                }
            } else {
                ui.label("Save this draft before retaining source images.");
            }
        }
    });
}

fn paragraph_excerpt(paragraph: &str) -> String {
    let mut text: String = paragraph.chars().take(120).collect();
    if paragraph.chars().count() > 120 {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        // Tests use the bundled egui face, without depending on macOS font files.
        let mut fonts = egui::FontDefinitions::default();
        let proportional = fonts.families[&egui::FontFamily::Proportional].clone();
        for name in ["Georgia", "Baskerville", "Avenir Next", "Helvetica Neue"] {
            fonts
                .families
                .insert(egui::FontFamily::Name(name.into()), proportional.clone());
        }
        ctx.set_fonts(fonts);
        ctx.set_theme(egui::Theme::Light);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.);
        ctx
    }

    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&mut Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 2200.),
                )),
                events,
                ..Default::default()
            },
            |root_ui| {
                egui::CentralPanel::default().show(root_ui, |ui| draw(ui));
            },
        );
        // Headless tests inspect paint/action output without a GPU renderer.
        output.textures_delta.clear();
        output
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn editor_frame(
        ctx: &egui::Context,
        state: &mut State,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<Action>) {
        let mut actions = Vec::new();
        let output = frame(ctx, events, |ui| editor(ui, state, &mut actions));
        (output, actions)
    }

    fn shelf_frame(
        ctx: &egui::Context,
        width: f32,
        issues: &[Magazine],
        state: &mut State,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<Action>) {
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 800.),
                )),
                events,
                ..Default::default()
            },
            |root_ui| {
                egui::Panel::left("navigation")
                    .exact_size(64.)
                    .resizable(false)
                    .frame(egui::Frame::NONE)
                    .show(root_ui, |_| {});
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(
                        if width < 1050. { 28 } else { 48 },
                        36,
                    )))
                    .show(root_ui, |ui| draw(ui, issues, state, &mut actions));
            },
        );
        output.textures_delta.clear();
        (output, actions)
    }

    #[test]
    fn shelf_metadata_flows_below_covers_and_opens_from_metadata_at_desktop_and_narrow_widths() {
        fn collect(
            shape: &egui::Shape,
            covers: &mut Vec<egui::Rect>,
            texts: &mut Vec<(String, egui::Rect)>,
        ) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == egui::Color32::from_rgb(242, 238, 228) => {
                    covers.push(rect.rect);
                }
                egui::Shape::Text(text) => texts.push((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, covers, texts);
                    }
                }
                _ => {}
            }
        }
        let issues = (1..=3)
            .map(|number| Magazine {
                id: MagazineId(format!("synthetic-edition-{number}")),
                title: format!("Quiet reading edition {number}"),
                subtitle: String::new(),
                created_at: "2026-10-22T00:00:00Z".into(),
                finalized_at: None,
                issue_number: number,
                paper_size: PaperSize::Letter,
                page_count: Some(24),
            })
            .collect::<Vec<_>>();
        for width in [1280., 800.] {
            let ctx = context();
            let mut state = State::default();
            // Settle the Grid's measured columns before deriving click positions.
            for _ in 0..3 {
                shelf_frame(&ctx, width, &issues, &mut state, Vec::new());
            }
            let (output, _) = shelf_frame(&ctx, width, &issues, &mut state, Vec::new());
            let mut covers = Vec::new();
            let mut texts = Vec::new();
            for shape in &output.shapes {
                collect(&shape.shape, &mut covers, &mut texts);
            }
            let cover = covers.first().expect("first edition cover");
            let title = texts
                .iter()
                .find(|(text, rect)| text == &issues[0].title && rect.top() > cover.bottom())
                .map(|(_, rect)| rect)
                .expect("edition title below cover, not beside it");
            let metadata = texts
                .iter()
                .find(|(text, _)| text.starts_with("Vol. 1 ·"))
                .map(|(_, rect)| rect)
                .expect("volume/date metadata");
            assert!(cover.width() > 250., "finite card width at {width}");
            assert!((title.left() - cover.left()).abs() < 1.);
            assert!(metadata.top() >= title.bottom());
            assert!(
                metadata.width() > 80. && metadata.height() < 24.,
                "metadata must remain a readable line at {width}: {metadata:?}"
            );
            assert!(metadata.right() <= cover.right());
            assert!(metadata.bottom() < 800., "first card metadata is visible");
            let click = metadata.center();
            shelf_frame(&ctx, width, &issues, &mut state, pointer(click, true));
            let (_, actions) = shelf_frame(&ctx, width, &issues, &mut state, pointer(click, false));
            assert!(
                actions.iter().any(|action| matches!(action,
                    Action::Command(Command::Open(id)) if id == &issues[0].id
                )),
                "clicking card metadata opens that edition at {width}"
            );
        }
    }

    #[test]
    fn selecting_theme_stays_local_until_save_and_save_keeps_latest_design() {
        let ctx = context();
        let mut state = State::default();
        state.start_manual(vec![ArticleId("synthetic-article".into())], &[]);
        let (output, actions) = editor_frame(&ctx, &mut state, Vec::new());
        assert!(actions.is_empty());
        let theme = text_position(&output, "Literary Review").expect("visible theme card");
        let (_, actions) = editor_frame(&ctx, &mut state, pointer(theme, true));
        assert!(actions.is_empty());
        let (output, actions) = editor_frame(&ctx, &mut state, pointer(theme, false));
        assert!(actions.is_empty(), "a design choice must not write a draft");
        assert_eq!(
            state.editing.as_ref().unwrap().design.theme.name,
            "Literary Review"
        );
        let save = text_position(&output, "Save ordered edition").expect("visible Save action");
        let (_, actions) = editor_frame(&ctx, &mut state, pointer(save, true));
        assert!(actions.is_empty());
        let (_, actions) = editor_frame(&ctx, &mut state, pointer(save, false));
        let saved = actions
            .iter()
            .find_map(|action| match action {
                Action::Command(Command::Save(input)) => Some(input),
                _ => None,
            })
            .expect("Save emits the prepared draft");
        assert_eq!(saved.design.theme.name, "Literary Review");
        assert_eq!(saved.design.theme.body_font, Font::Baskerville);
        assert_eq!(
            saved.article_ids,
            vec![ArticleId("synthetic-article".into())]
        );
        assert!(state.loading);
    }

    fn edition() -> Edition {
        Edition {
            issue: Issue {
                id: MagazineId("synthetic-edition".into()),
                title: "A synthetic edition".into(),
                subtitle: String::new(),
                paper_size: PaperSize::Letter,
                design: Design::default(),
                created_at: "2026-10-21T00:00:00Z".into(),
                finalized_at: None,
                issue_number: 1,
                articles: Vec::new(),
            },
            production: None,
            artifact: Some(Artifact {
                sha256: "current-exact-pdf-checksum".into(),
                page_count: 4,
                reviewed: false,
            }),
            historical: false,
            cover_request: Some(CoverRequest {
                id: "synthetic-request".into(),
                input_sha256: "synthetic-input".into(),
                state: "ready".into(),
                request: serde_json::json!({}),
                front_asset: None,
                back_asset: None,
            }),
            can_export_remove_pdf: false,
            exported_pdf: None,
        }
    }

    fn workflow_frame(
        ctx: &egui::Context,
        state: &mut State,
        edition: &Edition,
        busy: bool,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<Action>) {
        let mut actions = Vec::new();
        let output = frame(ctx, events, |ui| {
            production_workflow(ui, state, edition, busy, &mut actions)
        });
        (output, actions)
    }

    #[test]
    fn review_uses_exact_checksum_and_finalize_requires_review_and_idle() {
        let ctx = context();
        let mut state = State::default();
        let mut edition = edition();
        let (output, _) = workflow_frame(&ctx, &mut state, &edition, false, Vec::new());
        assert!(
            text_position(&output, "I reviewed this exact PDF").is_none(),
            "a PDF must be opened before acknowledging it"
        );
        assert!(
            text_position(&output, "Finalize edition").is_none(),
            "finalization is not the next action for an unreviewed PDF"
        );
        assert_eq!(
            next_step(&edition, None, edition.cover_request.as_ref()),
            NextStep::Open
        );
        state.opened_pdf_sha256 = Some("current-exact-pdf-checksum".into());
        let (output, _) = workflow_frame(&ctx, &mut state, &edition, false, Vec::new());
        let review = text_position(&output, "I reviewed this exact PDF").unwrap();
        workflow_frame(&ctx, &mut state, &edition, false, pointer(review, true));
        let (_, actions) =
            workflow_frame(&ctx, &mut state, &edition, false, pointer(review, false));
        assert!(actions.iter().any(|action| matches!(action, Action::Command(Command::Reviewed { id, sha256 }) if id == &edition.issue.id && sha256 == "current-exact-pdf-checksum")));
        edition.artifact.as_mut().unwrap().reviewed = true;
        let (output, _) = workflow_frame(&ctx, &mut state, &edition, true, Vec::new());
        let finalize = text_position(&output, "Finalize edition").unwrap();
        workflow_frame(&ctx, &mut state, &edition, true, pointer(finalize, true));
        let (_, actions) =
            workflow_frame(&ctx, &mut state, &edition, true, pointer(finalize, false));
        assert!(actions.is_empty(), "a running edition cannot finalize");
        workflow_frame(&ctx, &mut state, &edition, false, Vec::new());
        workflow_frame(&ctx, &mut state, &edition, false, pointer(finalize, true));
        let (_, actions) =
            workflow_frame(&ctx, &mut state, &edition, false, pointer(finalize, false));
        assert!(actions.iter().any(|action| matches!(action, Action::Command(Command::Finalize(id)) if id == &edition.issue.id)));
    }

    #[test]
    fn generation_preserves_the_explicit_partial_copy_acknowledgement() {
        let ctx = context();
        let mut state = State::default();
        let mut edition = edition();
        edition.artifact = None;
        let (output, _) = workflow_frame(&ctx, &mut state, &edition, false, Vec::new());
        let generate = text_position(&output, "Generate PDF").unwrap();
        workflow_frame(&ctx, &mut state, &edition, false, pointer(generate, true));
        let (output, actions) =
            workflow_frame(&ctx, &mut state, &edition, false, pointer(generate, false));
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::Command(Command::Queue {
                acknowledged: false,
                ..
            })
        )));
        let acknowledge = text_position(
            &output,
            "I acknowledge any partial or failed reading copies in this edition",
        )
        .unwrap();
        workflow_frame(
            &ctx,
            &mut state,
            &edition,
            false,
            pointer(acknowledge, true),
        );
        let (_, actions) = workflow_frame(
            &ctx,
            &mut state,
            &edition,
            false,
            pointer(acknowledge, false),
        );
        assert!(actions.is_empty());
        assert!(state.acknowledged);
        workflow_frame(&ctx, &mut state, &edition, false, pointer(generate, true));
        let (_, actions) =
            workflow_frame(&ctx, &mut state, &edition, false, pointer(generate, false));
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::Command(Command::Queue {
                acknowledged: true,
                ..
            })
        )));
    }
    #[test]
    fn finalized_historical_view_has_no_edit_finalize_or_protected_removal_action() {
        let ctx = context();
        let mut edition = edition();
        edition.issue.finalized_at = Some("2026-10-21T01:00:00Z".into());
        edition.historical = true;
        edition.can_export_remove_pdf = false;
        let mut state = State {
            edition: Some(edition),
            ..Default::default()
        };
        let mut actions = Vec::new();
        let output = frame(&ctx, Vec::new(), |ui| review(ui, &mut state, &mut actions));
        for label in [
            "Edit edition & design",
            "Generate PDF",
            "Regenerate / retry PDF",
            "Finalize edition",
        ] {
            assert!(
                text_position(&output, label).is_none(),
                "finalized view exposed {label}"
            );
        }
        let details = text_position(&output, "Edition details & storage").unwrap();
        frame(&ctx, pointer(details, true), |ui| {
            review(ui, &mut state, &mut actions)
        });
        let output = frame(&ctx, pointer(details, false), |ui| {
            review(ui, &mut state, &mut actions)
        });
        assert!(text_position(&output, "Export PDF and free local space…").is_none());
        assert!(text_position(&output, "Delete draft").is_none());
        assert!(actions.iter().all(|action| matches!(
            action,
            Action::Command(Command::Cover(_) | Command::Previews { .. })
        )));
    }
    #[test]
    fn regenerating_pdf_requires_opening_the_new_checksum_before_review() {
        let ctx = context();
        let original = edition();
        let mut state = State {
            opened_pdf_sha256: Some("current-exact-pdf-checksum".into()),
            edition: Some(original.clone()),
            ..Default::default()
        };
        let mut replacement = original;
        replacement.artifact.as_mut().unwrap().sha256 = "replacement-pdf-checksum".into();
        state.receive(Event::Edition(Box::new(replacement)));
        let current = state.edition.clone().unwrap();
        let (output, actions) = workflow_frame(&ctx, &mut state, &current, false, Vec::new());
        assert!(actions.is_empty());
        assert!(
            text_position(&output, "I reviewed this exact PDF").is_none(),
            "opening the previous PDF cannot acknowledge its replacement"
        );
        state.opened_pdf_sha256 = Some("replacement-pdf-checksum".into());
        let (output, _) = workflow_frame(&ctx, &mut state, &current, false, Vec::new());
        let review = text_position(&output, "I reviewed this exact PDF").unwrap();
        workflow_frame(&ctx, &mut state, &current, false, pointer(review, true));
        let (_, actions) =
            workflow_frame(&ctx, &mut state, &current, false, pointer(review, false));
        assert!(actions.iter().any(|action| matches!(action, Action::Command(Command::Reviewed { sha256, .. }) if sha256 == "replacement-pdf-checksum")));
    }
    #[test]
    fn preview_results_are_scoped_to_both_edition_and_generation_without_clearing_work() {
        let current = edition();
        let id = current.issue.id.clone();
        let mut state = State {
            edition: Some(current),
            preview_generation: 3,
            loading: true,
            ..Default::default()
        };
        let asset = || AssetPreview {
            key: "cover:front".into(),
            label: "Front".into(),
            sha256: "synthetic".into(),
            verified: true,
            raster: Some(CoverRaster {
                width: 1,
                height: 1,
                rgba: vec![255, 255, 255, 255],
            }),
            error: None,
        };
        state.receive(Event::Previews {
            id: id.clone(),
            generation: 2,
            assets: vec![asset()],
            provenance: None,
        });
        assert!(state.previews.is_empty());
        assert!(state.loading);
        state.receive(Event::Previews {
            id: MagazineId("other-edition".into()),
            generation: 3,
            assets: vec![asset()],
            provenance: None,
        });
        assert!(state.previews.is_empty());
        state.receive(Event::Previews {
            id,
            generation: 3,
            assets: vec![asset()],
            provenance: Some("proof".into()),
        });
        assert!(state.previews["cover:front"].verified);
        assert!(state.loading);
        assert_eq!(state.provenance.as_deref(), Some("proof"));
    }
    #[test]
    fn exclusions_search_filters_real_rows_and_allow_keeps_the_article_identity() {
        let ctx = context();
        let mut actions = Vec::new();
        let mut state = State {
            exclusions_open: true,
            policies: Some(Policies {
                articles: vec![
                    (ArticleId("alpine-id".into()), "Alpine story".into()),
                    (ArticleId("beta-id".into()), "Beta story".into()),
                ],
                publications: Vec::new(),
            }),
            ..Default::default()
        };
        frame(&ctx, Vec::new(), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        let output = frame(&ctx, Vec::new(), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        let search = text_position(&output, "Search articles or publication hostnames").unwrap();
        frame(&ctx, pointer(search, true), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        frame(&ctx, pointer(search, false), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        frame(&ctx, vec![egui::Event::Text("Alpine".into())], |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        let output = frame(&ctx, Vec::new(), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        assert_eq!(state.policy_filter, "Alpine");
        assert!(text_position(&output, "Alpine story").is_some());
        assert!(text_position(&output, "Beta story").is_none());
        let allow = text_position(&output, "Allow article").unwrap();
        frame(&ctx, pointer(allow, true), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        frame(&ctx, pointer(allow, false), |ui| {
            exclusions(ui, &mut state, &mut actions)
        });
        assert!(actions.iter().any(|action|matches!(action,Action::Command(Command::Exclude{id,excluded:false})if id.0=="alpine-id")));
    }
    #[test]
    fn drag_order_keeps_whole_article_identity_and_reaches_both_ends() {
        let mut ids = vec![
            ArticleId("a".into()),
            ArticleId("b".into()),
            ArticleId("c".into()),
        ];
        move_article(&mut ids, &ArticleId("c".into()), 0);
        assert_eq!(
            ids.iter().map(|id| id.0.as_str()).collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
        move_article(&mut ids, &ArticleId("c".into()), 3);
        assert_eq!(
            ids.iter().map(|id| id.0.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        move_article(&mut ids, &ArticleId("missing".into()), 0);
        assert_eq!(ids.len(), 3);
    }
    #[test]
    fn changed_pdf_cannot_submit_an_old_removal_confirmation() {
        let ctx = context();
        let mut edition = edition();
        edition.can_export_remove_pdf = true;
        let mut state = State {
            remove_pdf_confirm: Some((edition.issue.id.clone(), "old-pdf".into())),
            ..Default::default()
        };
        let mut actions = Vec::new();
        frame(&ctx, Vec::new(), |ui| {
            confirmations(ui, &mut state, &edition, &mut actions)
        });
        assert!(state.remove_pdf_confirm.is_none());
        assert!(actions.is_empty());
    }
    #[test]
    fn manual_draft_has_readable_metadata_before_any_save_and_dirty_state_is_real() {
        let ctx = context();
        let id = ArticleId("private-synthetic-id".into());
        let article = Article {
            id: id.clone(),
            url: "https://example.com/long-read".into(),
            title: "A readable first-draft title".into(),
            author: None,
            site: "Example Journal".into(),
            logo_url: None,
            excerpt: String::new(),
            word_count: 690,
            status: crate::domain::CopyStatus::Saved,
            error: None,
            published_at: None,
            saved_at: "2026-10-22T00:00:00Z".into(),
            read_at: None,
            archived_at: None,
            deliberate: true,
            excluded: false,
            printed: 0,
            drafted: 0,
        };
        let mut state = State::default();
        state.start_manual(vec![id], &[article]);
        let (output, actions) = editor_frame(&ctx, &mut state, Vec::new());
        assert!(actions.is_empty());
        assert!(text_position(&output, "A readable first-draft title").is_some());
        assert!(text_position(&output, "private-synthetic-id").is_none());
        let input = state.editing.as_mut().unwrap();
        let baseline = draft_signature(input);
        input.design.body_size_pt += 1.;
        assert_ne!(baseline, draft_signature(input));
        input.design.body_size_pt -= 1.;
        assert_eq!(baseline, draft_signature(input));
    }
    fn captured_repair() -> (State, RepairContext, RepairAsset) {
        let article = ArticleId("original-article".into());
        let other = ArticleId("other-article".into());
        let magazine = MagazineId("original-draft".into());
        let mut state = State::default();
        state.start_manual(vec![article.clone(), other], &[]);
        state.editing.as_mut().unwrap().id = Some(magazine.clone());
        let context = RepairContext {
            request_id: uuid::Uuid::new_v4(),
            edit_generation: state.edit_generation,
            magazine_id: magazine,
            article_id: article,
            after_text: "Original unique paragraph.".into(),
            alt: "Captured original alt".into(),
            note: "Captured source note".into(),
        };
        let asset = RepairAsset {
            id: "retained-original".into(),
            sha256: "verified-original-sha".into(),
            source_url: "https://example.com/source".into(),
            image_url: "https://example.com/image.png".into(),
            retrieved_at: "2026-10-22T00:00:00Z".into(),
        };
        assert!(state.repair_started(&context));
        (state, context, asset)
    }
    #[test]
    fn repair_response_keeps_captured_article_and_placement_after_reorder_and_form_edits() {
        let (mut state, context, asset) = captured_repair();
        state.editing.as_mut().unwrap().article_ids.swap(0, 1);
        state.repair_article = 0;
        state.repair_anchor = "A different live paragraph".into();
        state.repair_alt = "A different live alt".into();
        state.repair_note = "A different live note".into();
        let response = Event::Repair(RepairResult {
            context: Some(Box::new(context.clone())),
            asset: asset.clone(),
        });
        state.receive(response.clone());
        state.receive(response);
        let input = state.editing.as_ref().unwrap();
        assert!(
            !input
                .presentation
                .contains_key(&ArticleId("other-article".into()))
        );
        let insertions = &input.presentation[&context.article_id].insertions;
        assert_eq!(
            insertions.len(),
            1,
            "duplicate response must not insert twice"
        );
        assert_eq!(insertions[0].after_text, context.after_text);
        assert_eq!(insertions[0].alt, context.alt);
        assert_eq!(insertions[0].note, context.note);
        assert_eq!(insertions[0].asset_id, asset.id);
    }
    #[test]
    fn repair_responses_cannot_cross_edit_sessions_drafts_or_removed_articles() {
        let (mut state, context, asset) = captured_repair();
        state.start_manual(vec![context.article_id.clone()], &[]);
        state.editing.as_mut().unwrap().id = Some(context.magazine_id.clone());
        state.loading = true;
        state.receive(Event::Repair(RepairResult {
            context: Some(Box::new(context.clone())),
            asset: asset.clone(),
        }));
        assert!(state.editing.as_ref().unwrap().presentation.is_empty());
        assert!(state.loading, "stale work cannot clear unrelated loading");
        let (mut state, context, asset) = captured_repair();
        state.editing.as_mut().unwrap().id = Some(MagazineId("different-draft".into()));
        state.receive(Event::Repair(RepairResult {
            context: Some(Box::new(context)),
            asset,
        }));
        assert!(state.editing.as_ref().unwrap().presentation.is_empty());
        let (mut state, context, asset) = captured_repair();
        state
            .editing
            .as_mut()
            .unwrap()
            .article_ids
            .retain(|id| id != &context.article_id);
        state.receive(Event::Repair(RepairResult {
            context: Some(Box::new(context)),
            asset,
        }));
        assert!(state.editing.as_ref().unwrap().presentation.is_empty());
        let (_, context, asset) = captured_repair();
        let mut closed = State::default();
        closed.receive(Event::Repair(RepairResult {
            context: Some(Box::new(context)),
            asset,
        }));
        assert!(closed.editing.is_none());
        assert!(closed.previews.is_empty());
    }
    #[test]
    fn editor_cannot_save_before_pending_repair_is_attached_and_then_saves_it() {
        let ctx = context();
        let (mut state, context, asset) = captured_repair();
        let (output, _) = editor_frame(&ctx, &mut state, Vec::new());
        let save = text_position(&output, "Save ordered edition").unwrap();
        editor_frame(&ctx, &mut state, pointer(save, true));
        let (_, actions) = editor_frame(&ctx, &mut state, pointer(save, false));
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, Action::Command(Command::Save(_)))),
            "a pending original must not produce a pre-receipt draft input"
        );
        assert_eq!(
            state.relevant_pending_repairs(state.editing.as_ref().unwrap()),
            1
        );
        state.receive(Event::Repair(RepairResult {
            context: Some(Box::new(context.clone())),
            asset,
        }));
        assert!(state.can_save(state.editing.as_ref().unwrap()));
        let (output, _) = editor_frame(&ctx, &mut state, Vec::new());
        let save = text_position(&output, "Save ordered edition").unwrap();
        editor_frame(&ctx, &mut state, pointer(save, true));
        let (_, actions) = editor_frame(&ctx, &mut state, pointer(save, false));
        let saved = actions
            .iter()
            .find_map(|action| match action {
                Action::Command(Command::Save(input)) => Some(input),
                _ => None,
            })
            .expect("completion enables Save");
        assert_eq!(
            saved.presentation[&context.article_id].insertions[0].after_text,
            context.after_text
        );
        assert_eq!(
            saved.presentation[&context.article_id].insertions[0].alt,
            context.alt
        );
    }
    #[test]
    fn repair_failures_release_only_their_registered_request_and_ignore_stale_errors() {
        let (mut state, first, asset) = captured_repair();
        let mut second = first.clone();
        second.request_id = uuid::Uuid::new_v4();
        second.article_id = ArticleId("other-article".into());
        assert!(state.repair_started(&second));
        let mut mismatch = first.clone();
        mismatch.note = "not the registered immutable context".into();
        state.receive(Event::RepairFailed {
            context: Some(Box::new(mismatch)),
            message: "stale mismatch".into(),
        });
        assert_eq!(state.pending_repairs.len(), 2);
        assert!(state.repair_error.is_none());
        state.receive(Event::RepairFailed {
            context: Some(Box::new(first.clone())),
            message: "Synthetic original could not be read".into(),
        });
        assert_eq!(state.pending_repairs.len(), 1);
        assert!(state.pending_repairs.contains_key(&second.request_id));
        assert!(!state.can_save(state.editing.as_ref().unwrap()));
        assert_eq!(
            state.repair_error.as_ref().unwrap().request_id,
            first.request_id
        );
        state.receive(Event::Repair(RepairResult {
            context: Some(Box::new(second)),
            asset,
        }));
        assert!(state.can_save(state.editing.as_ref().unwrap()));
        assert_eq!(
            state.repair_error.as_ref().unwrap().request_id,
            first.request_id,
            "another success cannot erase this failure"
        );
        state.start_manual(vec![first.article_id.clone()], &[]);
        state.editing.as_mut().unwrap().id = Some(first.magazine_id.clone());
        state.receive(Event::RepairFailed {
            context: Some(Box::new(first)),
            message: "late old edit failure".into(),
        });
        assert!(state.pending_repairs.is_empty());
        assert!(state.repair_error.is_none());
    }
}
