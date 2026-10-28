use crate::{db_compat, domain::*, library};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Import,
    Export,
}

#[derive(Debug)]
pub enum TransferResult {
    ImportPreview(Vec<crate::imports::Progress>),
    Exported,
}

#[derive(Clone, Debug)]
pub enum Command {
    List {
        generation: u64,
        query: Query,
        offset: usize,
    },
    Reconcile {
        generation: u64,
        query: Query,
        loaded: usize,
    },
    Body {
        generation: u64,
        id: ArticleId,
    },
    CancelBody,
    RefreshBody {
        generation: u64,
        id: ArticleId,
    },
    RefreshSelected {
        generation: u64,
        id: ArticleId,
    },
    FetchedMetadata(Box<Article>),
    PreparedDraft {
        request: u64,
        expected_edition: Option<String>,
        input: Box<crate::magazine::DraftInput>,
        prepared: Vec<crate::magazine::PreparedArticle>,
    },
    PreparedPlans {
        request: u64,
        plans: crate::magazine::Plans,
    },
    AcquisitionProgress {
        request: u64,
        message: String,
    },
    AcquisitionFailed {
        request: u64,
        message: String,
    },
    Summary,
    Save(String),
    Read {
        id: ArticleId,
        read: bool,
    },
    Archive {
        id: ArticleId,
        archived: bool,
    },
    Delete(Vec<ArticleId>),
    DeleteMatching {
        query: Query,
        expected: u32,
    },
    Refetch(ArticleId),
    RefetchAll,
    AddFeed(String),
    Feed {
        id: FeedId,
        action: FeedAction,
    },
    Pdf(MagazineId),
    Imports,
    StageImport {
        request_id: TransferId,
        path: PathBuf,
    },
    StartImport {
        id: String,
        retry: bool,
    },
    RemoveImport(String),
    Export {
        request_id: TransferId,
        path: PathBuf,
    },
    Magazine(crate::magazine::Command),
    CoverFiles {
        magazine_id: MagazineId,
        request_id: String,
        input_sha256: String,
        front: PathBuf,
        back: PathBuf,
    },
    RepairFile {
        input: crate::magazine::RepairInput,
        path: PathBuf,
    },
    ExportCoverRequest {
        request: crate::magazine::CoverRequest,
        path: PathBuf,
    },
    Quit,
}
#[derive(Debug)]
pub enum Event {
    List {
        generation: u64,
        offset: usize,
        articles: Vec<Article>,
        publications: Vec<(String, u32)>,
        logos: Vec<(String, Vec<u8>)>,
        total: u32,
        has_more: bool,
    },
    Logos {
        generation: u64,
        logos: Vec<(String, Vec<u8>)>,
    },
    Body {
        generation: u64,
        body: Box<ArticleBody>,
        fetched_at: Option<String>,
    },
    BodyError {
        generation: u64,
        id: ArticleId,
        message: String,
    },
    SelectedMetadata {
        generation: u64,
        article: Option<Article>,
    },
    MagazineProgress(String),
    Summary {
        counts: Counts,
        feeds: Vec<Feed>,
        magazines: Vec<Magazine>,
    },
    Pdf {
        id: MagazineId,
        title: String,
        bytes: Vec<u8>,
    },
    Imports(Vec<crate::imports::Progress>),
    TransferFinished {
        request_id: TransferId,
        result: Result<TransferResult, String>,
    },
    Magazine(crate::magazine::Event),
    Changed,
    Notice(String),
    Error(String),
}
pub struct Backend {
    pub commands: SyncSender<Command>,
    pub events: Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Backend {
    pub fn spawn(path: PathBuf, ctx: egui::Context, offline: bool) -> Self {
        let (commands, rx) = mpsc::sync_channel(64);
        let (tx, events) = mpsc::sync_channel(64);
        let acquired_commands = commands.clone();
        let worker = thread::spawn(move || {
            let mut c = match db_compat::open(&path).and_then(|c| {
                db_compat::initialize(&c)?;
                Ok(c)
            }) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Event::Error(format!("{e:#}")));
                    ctx.request_repaint();
                    return;
                }
            };
            let mut reader = crate::loader::spawn(path.clone(), offline, tx.clone(), ctx.clone());
            let mut icons = crate::icons::spawn(path.clone(), offline, tx.clone(), ctx.clone());
            let mut magazines = MagazineAcquirer::spawn(
                path.clone(),
                offline,
                acquired_commands,
                tx.clone(),
                ctx.clone(),
            );
            let mut importing = true;
            let mut acquisition_generation = 0;
            loop {
                let command = if importing {
                    match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                        Ok(c) => Some(c),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                } else {
                    match rx.recv() {
                        Ok(c) => Some(c),
                        Err(_) => break,
                    }
                };
                let Some(command) = command else {
                    match crate::imports::run_batch(&mut c) {
                        Ok(ran) => {
                            importing = crate::imports::active(&c).unwrap_or(false);
                            if ran {
                                let progress = crate::imports::list(&c);
                                if let Ok(progress) = progress {
                                    if tx.send(Event::Imports(progress)).is_err() {
                                        return;
                                    }
                                    ctx.request_repaint();
                                }
                                if tx.send(Event::Changed).is_err() {
                                    return;
                                }
                                ctx.request_repaint();
                            }
                        }
                        Err(e) => {
                            importing = false;
                            if let Ok(progress) = crate::imports::list(&c) {
                                let _ = tx.send(Event::Imports(progress));
                            }
                            if tx
                                .send(Event::Error(format!("Import stopped: {e:#}")))
                                .is_err()
                            {
                                return;
                            }
                            ctx.request_repaint();
                        }
                    }
                    continue;
                };
                if matches!(command, Command::Quit) {
                    break;
                }
                if !acquisition_is_current(&command, acquisition_generation) {
                    continue;
                }
                if let Command::Body { generation, id } = command {
                    if reader
                        .commands
                        .try_send(crate::loader::Command::Open {
                            generation,
                            id: id.clone(),
                            refresh: false,
                        })
                        .is_err()
                    {
                        let _ = tx.send(Event::BodyError {
                            generation,
                            id,
                            message: "Reader queue is full. Retry the article.".into(),
                        });
                        ctx.request_repaint();
                    }
                    continue;
                }
                if let Command::RefreshBody { generation, id } = command {
                    if reader
                        .commands
                        .try_send(crate::loader::Command::Open {
                            generation,
                            id: id.clone(),
                            refresh: true,
                        })
                        .is_err()
                    {
                        let _ = tx.send(Event::BodyError {
                            generation,
                            id,
                            message: "Reader queue is full. Retry the article.".into(),
                        });
                        ctx.request_repaint();
                    }
                    continue;
                }
                if matches!(command, Command::CancelBody) {
                    let _ = reader.commands.try_send(crate::loader::Command::Cancel);
                    continue;
                }
                if db_compat::is_lean(&c).unwrap_or(false)
                    && matches!(
                        &command,
                        Command::Magazine(
                            crate::magazine::Command::Plans | crate::magazine::Command::Save(_)
                        )
                    )
                {
                    acquisition_generation += 1;
                    let Command::Magazine(command) = command else {
                        unreachable!()
                    };
                    let expected_edition = match &command {
                        crate::magazine::Command::Save(input) => input
                            .id
                            .as_ref()
                            .map(|id| edition_fingerprint(&c, id))
                            .transpose(),
                        _ => Ok(None),
                    };
                    match expected_edition {
                        Ok(expected_edition) => {
                            if magazines
                                .commands
                                .try_send(AcquisitionCommand::Run {
                                    request: acquisition_generation,
                                    expected_edition,
                                    command,
                                })
                                .is_err()
                            {
                                let _ = tx.send(Event::Error(
                                    "Magazine acquisition queue is full".into(),
                                ));
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(Event::Error(format!(
                                "Edition could not be acquired: {error:#}"
                            )));
                        }
                    }
                    ctx.request_repaint();
                    continue;
                }
                if matches!(command, Command::StartImport { .. }) {
                    importing = true;
                }
                let result = dispatch(&mut c, command);
                match result {
                    Ok(events) => {
                        for mut event in events {
                            if let Event::List {
                                generation,
                                articles,
                                ..
                            } = &mut event
                                && db_compat::is_lean(&c).unwrap_or(false)
                            {
                                let requests = crate::icons::requests(articles);
                                for article in articles {
                                    article.logo_url = crate::icons::identity(article);
                                }
                                icons.batch(*generation, requests);
                            }
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        if tx.send(Event::Error(format!("{e:#}"))).is_err() {
                            return;
                        }
                    }
                }
                ctx.request_repaint();
            }
            reader.shutdown();
            icons.shutdown();
            magazines.shutdown();
        });
        Self {
            commands,
            events,
            thread: Some(worker),
        }
    }
    pub fn send(&self, cmd: Command) -> Result<(), String> {
        self.commands
            .try_send(cmd)
            .map_err(|e| format!("Database command could not be queued: {e}"))
    }
    pub fn shutdown(&mut self) {
        let (_, empty) = mpsc::sync_channel(1);
        drop(std::mem::replace(&mut self.events, empty));
        let _ = self.commands.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn dispatch(c: &mut rusqlite::Connection, cmd: Command) -> anyhow::Result<Vec<Event>> {
    let event = match cmd {
        Command::List {
            generation,
            query,
            offset,
        } => Event::List {
            generation,
            offset,
            articles: library::articles(c, &query, offset)?,
            logos: library::logos(c, &library::articles(c, &query, offset)?)?,
            publications: library::publications(c, &query)?,
            total: library::matching_count(c, &query)?,
            has_more: offset + 60 < (library::matching_count(c, &query)? as usize),
        },
        Command::Reconcile {
            generation,
            query,
            loaded,
        } => {
            let snapshot = c.unchecked_transaction()?;
            let total = library::matching_count(&snapshot, &query)?;
            let mut articles = vec![];
            for offset in (0..loaded.clamp(60, 1_000_000)).step_by(60) {
                let page = library::articles(&snapshot, &query, offset)?;
                let done = page.len() < 60;
                articles.extend(page);
                if done {
                    break;
                }
            }
            let has_more = articles.len() < (total as usize);
            Event::List {
                generation,
                offset: 0,
                logos: library::logos(&snapshot, &articles)?,
                articles,
                publications: library::publications(&snapshot, &query)?,
                total,
                has_more,
            }
        }
        Command::Body { generation, id } => Event::Body {
            generation,
            body: Box::new(library::body(c, &id)?),
            fetched_at: None,
        },
        Command::CancelBody => return Ok(vec![]),
        Command::RefreshBody { .. } => return Ok(vec![]),
        Command::RefreshSelected { generation, id } => {
            let article = match library::metadata(c, &id) {
                Ok(a) => Some(a),
                Err(e)
                    if e.downcast_ref::<rusqlite::Error>()
                        .is_some_and(|e| matches!(e, rusqlite::Error::QueryReturnedNoRows)) =>
                {
                    None
                }
                Err(e) => return Err(e),
            };
            Event::SelectedMetadata {
                generation,
                article,
            }
        }
        Command::FetchedMetadata(article) => {
            if db_compat::is_lean(c)? {
                let storage = crate::storage::Storage::for_database(c)?;
                let _reservation = storage.require(131072)?;
                let changed=c.execute("UPDATE articles SET title=?3,author=?4,site=?5,excerpt=?6,word_count=?7,status=?8,error=?9,published_at=COALESCE(?10,published_at) WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL AND (title IS NOT ?3 OR author IS NOT ?4 OR site IS NOT ?5 OR excerpt IS NOT ?6 OR word_count IS NOT ?7 OR status IS NOT ?8 OR error IS NOT ?9 OR published_at IS NOT COALESCE(?10,published_at))",rusqlite::params![OWNER,article.id.0,article.title,article.author,article.site,article.excerpt,article.word_count,article.status.as_str(),article.error,article.published_at])?;
                if changed > 0 {
                    return Ok(vec![Event::Changed]);
                }
            }
            return Ok(vec![]);
        }
        Command::PreparedDraft {
            expected_edition,
            input,
            prepared,
            ..
        } => {
            validate_prepared_edition(c, &input, expected_edition.as_deref())?;
            let id = crate::magazine::save_prepared_draft(c, &input, &prepared)?;
            Event::Magazine(crate::magazine::Event::Edition(Box::new(
                crate::magazine::edition(c, &id)?,
            )))
        }
        Command::PreparedPlans { plans, .. } => {
            Event::Magazine(crate::magazine::Event::Plans(plans))
        }
        Command::AcquisitionProgress { message, .. } => Event::MagazineProgress(message),
        Command::AcquisitionFailed { message, .. } => Event::Error(message),
        Command::Summary => Event::Summary {
            counts: library::counts(c)?,
            feeds: library::feeds(c)?,
            magazines: library::magazines(c)?,
        },
        Command::Save(url) => {
            library::save(c, &url, ArrivalKind::Web, &url, "Saved link", None, None)?;
            Event::Changed
        }
        Command::Read { id, read } => {
            library::update(c, &id, Some(read), None)?;
            Event::Changed
        }
        Command::Archive { id, archived } => {
            library::update(c, &id, None, Some(archived))?;
            Event::Changed
        }
        Command::Delete(ids) => {
            library::delete(c, &ids)?;
            Event::Changed
        }
        Command::DeleteMatching { query, expected } => {
            library::delete_matching(c, &query, expected)?;
            Event::Changed
        }
        Command::Refetch(id) => {
            library::enqueue(c, "article", &id.0)?;
            Event::Notice("Article queued for re-fetch".into())
        }
        Command::RefetchAll => {
            let n = library::refetch_all(c)?;
            Event::Notice(format!("{n} articles queued for re-fetch"))
        }
        Command::AddFeed(url) => {
            library::add_feed(c, &url)?;
            Event::Changed
        }
        Command::Feed { id, action } => {
            library::update_feed(c, &id, action)?;
            Event::Changed
        }
        Command::Pdf(id) => Event::Pdf {
            title: c.query_row("SELECT title FROM magazines WHERE id=?1", [&id.0], |row| {
                row.get(0)
            })?,
            bytes: library::stored_pdf(c, &id)?,
            id,
        },
        Command::Imports => Event::Imports(crate::imports::list(c)?),
        Command::StageImport { request_id, path } => Event::TransferFinished {
            request_id,
            result: crate::imports::stage(c, &path)
                .and_then(|_| crate::imports::list(c))
                .map(TransferResult::ImportPreview)
                .map_err(|error| format!("{error:#}")),
        },
        Command::StartImport { id, retry } => {
            crate::imports::start(c, &id, retry)?;
            Event::Imports(crate::imports::list(c)?)
        }
        Command::RemoveImport(id) => {
            crate::imports::remove(c, &id)?;
            Event::Imports(crate::imports::list(c)?)
        }
        Command::Export { request_id, path } => Event::TransferFinished {
            request_id,
            result: crate::exports::export(c, &path)
                .map(|_| TransferResult::Exported)
                .map_err(|error| format!("{error:#}")),
        },
        Command::CoverFiles {
            magazine_id,
            request_id,
            input_sha256,
            front,
            back,
        } => {
            let input = crate::magazine::CoverFulfilment {
                magazine_id,
                request_id,
                input_sha256,
                front: private_file(&front, 20 * 1024 * 1024)?,
                back: private_file(&back, 20 * 1024 * 1024)?,
                note: format!(
                    "Original artwork files selected locally: {} and {}",
                    front.file_name().unwrap_or_default().to_string_lossy(),
                    back.file_name().unwrap_or_default().to_string_lossy()
                ),
            };
            Event::Magazine(crate::magazine::execute(
                c,
                crate::magazine::Command::Fulfil(input),
            )?)
        }
        Command::RepairFile { mut input, path } => {
            let context = input.context.clone();
            let result = (|| {
                input.body = private_file(&path, 8 * 1024 * 1024)?;
                crate::magazine::execute(c, crate::magazine::Command::AddRepair(input))
            })();
            Event::Magazine(match result {
                Ok(event) => event,
                Err(error) => crate::magazine::Event::RepairFailed {
                    context,
                    message: error.to_string(),
                },
            })
        }
        Command::ExportCoverRequest { request, path } => {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let database = crate::storage::database_path(c)?;
            let executable = std::env::current_exe()?;
            let bundle = executable.ancestors().find(|ancestor| {
                ancestor
                    .extension()
                    .is_some_and(|extension| extension == "app")
            });
            let path = external_export_path(
                &path,
                database.as_deref().and_then(std::path::Path::parent),
                bundle,
            )?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(&serde_json::to_vec_pretty(&request)?)?;
            file.sync_all()?;
            Event::Notice(format!("Cover request saved to {}", path.display()))
        }
        Command::Magazine(cmd) => Event::Magazine(crate::magazine::execute(c, cmd)?),
        Command::Quit => return Ok(vec![]),
    };
    Ok(vec![event])
}

fn external_export_path(
    path: &std::path::Path,
    managed_root: Option<&std::path::Path>,
    bundle: Option<&std::path::Path>,
) -> anyhow::Result<PathBuf> {
    use anyhow::{Context, ensure};
    ensure!(path.is_absolute(), "Choose an absolute export path");
    let parent = path
        .parent()
        .context("Choose an export folder")?
        .canonicalize()?;
    let target = parent.join(path.file_name().context("Choose an export filename")?);
    for root in managed_root.into_iter().chain(bundle) {
        ensure!(
            !target.starts_with(root.canonicalize()?),
            "Export outside the managed library and installed application bundle"
        );
    }
    Ok(target)
}

fn acquisition_is_current(command: &Command, current: u64) -> bool {
    match command {
        Command::PreparedDraft { request, .. }
        | Command::PreparedPlans { request, .. }
        | Command::AcquisitionProgress { request, .. }
        | Command::AcquisitionFailed { request, .. } => *request == current,
        _ => true,
    }
}
fn edition_fingerprint(c: &rusqlite::Connection, id: &MagazineId) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let value = serde_json::to_value(crate::magazine::edition(c, id)?)?;
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}
fn validate_prepared_edition(
    c: &rusqlite::Connection,
    input: &crate::magazine::DraftInput,
    expected: Option<&str>,
) -> anyhow::Result<()> {
    match (&input.id, expected) {
        (None, None) => Ok(()),
        (Some(id), Some(expected)) => {
            anyhow::ensure!(
                edition_fingerprint(c, id)? == expected,
                "Edition changed while articles were being acquired. Review the current edition and save again."
            );
            Ok(())
        }
        _ => anyhow::bail!("Prepared edition is missing its original revision"),
    }
}
enum AcquisitionCommand {
    Run {
        request: u64,
        expected_edition: Option<String>,
        command: crate::magazine::Command,
    },
    Quit,
}
struct MagazineAcquirer {
    commands: tokio::sync::mpsc::Sender<AcquisitionCommand>,
    thread: Option<thread::JoinHandle<()>>,
}
struct OfflineFetcher;
#[async_trait::async_trait(?Send)]
impl crate::intake::Fetcher for OfflineFetcher {
    async fn fetch(&self, _url: &str, _image: bool) -> anyhow::Result<crate::network::Resource> {
        anyhow::bail!("Connect to the internet to acquire this article for production")
    }
}
impl MagazineAcquirer {
    fn spawn(
        path: PathBuf,
        offline: bool,
        db_commands: SyncSender<Command>,
        events: SyncSender<Event>,
        ctx: egui::Context,
    ) -> Self {
        let (commands, mut rx) = tokio::sync::mpsc::channel(8);
        let thread=thread::Builder::new().name("read-later-acquisition".into()).spawn(move||{
            let result=(||->anyhow::Result<()>{let c=db_compat::open_read_only(&path)?;let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                runtime.block_on(async {let mut pending=None;loop {
                    let command=match pending.take(){Some(c)=>Some(c),None=>rx.recv().await};
                    let Some(AcquisitionCommand::Run {request, expected_edition, command})=command else {break;};
                    let progress=|done,total|{let _=db_commands.try_send(Command::AcquisitionProgress {request,message:format!("Acquiring articles {done}/{total}")});};
                    let work=async {
                        let fetcher:&dyn crate::intake::Fetcher=if offline{&OfflineFetcher}else{&crate::intake::PublicFetcher};
                        match command {
                            crate::magazine::Command::Plans=>{
                                let plans=crate::magazine::acquire_plans(&c,fetcher,progress).await?;
                                db_commands.try_send(Command::PreparedPlans {request,plans}).map_err(|e|anyhow::anyhow!("Acquired proposals could not be queued: {e}"))?;
                            },
                            crate::magazine::Command::Save(input)=>{
                                let prepared=crate::magazine::acquire_articles(&c,&input,fetcher,progress).await?;
                                db_commands.try_send(Command::PreparedDraft{request,expected_edition,input,prepared}).map_err(|e|anyhow::anyhow!("Prepared edition could not be queued: {e}"))?;
                            },_=>{}
                        }
                        Ok::<_,anyhow::Error>(())
                    };tokio::pin!(work);
                    tokio::select!{next=rx.recv()=>{pending=next;},result=&mut work=>if let Err(error)=result{let _=db_commands.try_send(Command::AcquisitionFailed {request,message:format!("Magazine acquisition failed: {error:#}")});}}
                }});Ok(())})();
            if let Err(error)=result{let _=events.send(Event::Error(format!("Magazine acquisition stopped: {error:#}")));ctx.request_repaint();}
        }).expect("Magazine acquisition thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }
    fn shutdown(&mut self) {
        let _ = self.commands.blocking_send(AcquisitionCommand::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn private_file(path: &std::path::Path, limit: u64) -> anyhow::Result<Vec<u8>> {
    let m = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        m.is_file() && !m.file_type().is_symlink() && m.len() <= limit,
        "Selected artwork must be a regular file within the size limit"
    );
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "Selected artwork grew beyond the size limit"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_repair_file_reports_its_captured_request_without_a_global_error() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        db_compat::register(&connection).unwrap();
        db_compat::initialize(&connection).unwrap();
        let context = crate::magazine::RepairContext {
            request_id: uuid::Uuid::new_v4(),
            edit_generation: uuid::Uuid::new_v4(),
            magazine_id: MagazineId("synthetic-draft".into()),
            article_id: ArticleId("synthetic-article".into()),
            after_text: "Captured paragraph".into(),
            alt: "Captured alternative text".into(),
            note: "Captured source note".into(),
        };
        let events = dispatch(
            &mut connection,
            Command::RepairFile {
                input: crate::magazine::RepairInput {
                    context: Some(Box::new(context.clone())),
                    magazine_id: context.magazine_id.clone(),
                    body: Vec::new(),
                    label: "Synthetic source image".into(),
                    source_url: "https://example.com/source".into(),
                    image_url: "https://example.com/image.png".into(),
                    retrieved_at: "2026-10-22T00:00:00Z".into(),
                },
                path: directory.path().join("missing.png"),
            },
        )
        .unwrap();
        assert!(
            matches!(events.as_slice(), [Event::Magazine(crate::magazine::Event::RepairFailed {context: Some(actual), message})] if **actual == context && !message.is_empty())
        );
    }

    #[test]
    fn file_transfer_success_and_errors_keep_their_request_identity() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        db_compat::register(&connection).unwrap();
        db_compat::initialize(&connection).unwrap();
        for (request_id, command) in [
            (
                TransferId(17),
                Command::StageImport {
                    request_id: TransferId(17),
                    path: directory.path().join("missing.csv"),
                },
            ),
            (
                TransferId(18),
                Command::Export {
                    request_id: TransferId(18),
                    path: directory.path().join("missing-folder/library.zip"),
                },
            ),
        ] {
            let events = dispatch(&mut connection, command).unwrap();
            assert!(
                matches!(events.as_slice(), [Event::TransferFinished { request_id: actual, result: Err(message) }] if *actual == request_id && !message.is_empty())
            );
        }
        let input = directory.path().join("reading.csv");
        std::fs::write(
            &input,
            "url,title\nhttps://example.org/article,A synthetic article\n",
        )
        .unwrap();
        let events = dispatch(
            &mut connection,
            Command::StageImport {
                request_id: TransferId(19),
                path: input,
            },
        )
        .unwrap();
        assert!(
            matches!(events.as_slice(), [Event::TransferFinished { request_id: TransferId(19), result: Ok(TransferResult::ImportPreview(runs)) }] if runs.len() == 1 && runs[0].state == "preview" && runs[0].total == 1)
        );
    }

    #[test]
    fn cover_export_rejects_managed_bundle_and_symlinked_parent_before_writing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("managed");
        let bundle = dir.path().join("Reader.app");
        let external = dir.path().join("exports");
        for path in [&managed, &bundle, &external] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = external.join("library-link");
        std::os::unix::fs::symlink(&managed, &link).unwrap();
        for parent in [&managed, &bundle, &link] {
            let path = parent.join("cover-request.json");
            assert!(external_export_path(&path, Some(&managed), Some(&bundle)).is_err());
            assert!(!path.exists());
        }
        let mut c = db_compat::open(&managed.join("library.sqlite3")).unwrap();
        let request = crate::magazine::CoverRequest {
            id: "synthetic".into(),
            input_sha256: "a".repeat(64),
            state: "requested".into(),
            request: serde_json::json!({"prompt":"Complete synthetic cover context"}),
            front_asset: None,
            back_asset: None,
        };
        let target = external.join("cover-request.json");
        dispatch(
            &mut c,
            Command::ExportCoverRequest {
                request: request.clone(),
                path: target.clone(),
            },
        )
        .unwrap();
        let saved: crate::magazine::CoverRequest =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(saved.input_sha256, request.input_sha256);
        assert_eq!(saved.request, request.request);
        let blocked = managed.join("cover-request.json");
        assert!(
            dispatch(
                &mut c,
                Command::ExportCoverRequest {
                    request,
                    path: blocked.clone()
                }
            )
            .is_err()
        );
        assert!(!blocked.exists());
    }

    #[test]
    fn fetched_metadata_reconciles_rows_once_without_resetting_read_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = db_compat::open(&dir.path().join("library.sqlite3")).unwrap();
        db_compat::initialize_lean(&c).unwrap();
        let (id, _) = library::save(
            &mut c,
            "https://example.org/new",
            ArrivalKind::Web,
            "metadata-reconcile",
            "Saved",
            Some("Preview title"),
            None,
        )
        .unwrap();
        library::update(&c, &id, Some(true), None).unwrap();
        let mut article = library::metadata(&c, &id).unwrap();
        let read_at = article.read_at.clone();
        article.title = "Fetched title".into();
        article.excerpt = "Fetched preview".into();
        article.word_count = 690;
        article.status = CopyStatus::Saved;
        let events = dispatch(&mut c, Command::FetchedMetadata(Box::new(article.clone()))).unwrap();
        assert!(matches!(&events[..], [Event::Changed]));
        let row = library::metadata(&c, &id).unwrap();
        assert_eq!(row.title, "Fetched title");
        assert_eq!(row.minutes(), 3);
        assert_eq!(row.read_at, read_at);
        let revision: i64 = c
            .query_row(
                "SELECT library_revision FROM users WHERE id=?1",
                [OWNER],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            dispatch(&mut c, Command::FetchedMetadata(Box::new(article)))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            c.query_row(
                "SELECT library_revision FROM users WHERE id=?1",
                [OWNER],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            revision
        );
    }

    fn draft() -> (rusqlite::Connection, crate::magazine::DraftInput) {
        let mut c = rusqlite::Connection::open_in_memory().unwrap();
        db_compat::register(&c).unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db_compat::initialize(&c).unwrap();
        crate::magazine::initialize(&c).unwrap();
        let (article, _) = library::save(
            &mut c,
            "https://example.org/article",
            ArrivalKind::Web,
            "test",
            "Saved",
            Some("Title"),
            None,
        )
        .unwrap();
        c.execute("UPDATE articles SET html='<p>Exact complete snapshot ending.</p>',word_count=4,status='saved' WHERE id=?1",[&article.0]).unwrap();
        let mut input = crate::magazine::DraftInput {
            id: None,
            title: "Original edition".into(),
            subtitle: String::new(),
            paper_size: crate::magazine::PaperSize::Letter,
            article_ids: vec![article],
            refresh_copies: false,
            design: Default::default(),
            presentation: Default::default(),
        };
        input.id = Some(crate::magazine::save_draft(&mut c, &input).unwrap());
        (c, input)
    }

    #[test]
    fn prepared_save_rejects_changed_edition_before_snapshot_publication() {
        let (mut c, input) = draft();
        let id = input.id.as_ref().unwrap();
        let expected = edition_fingerprint(&c, id).unwrap();
        c.execute(
            "UPDATE magazines SET title='Newer title' WHERE id=?1",
            [&id.0],
        )
        .unwrap();
        let result = dispatch(
            &mut c,
            Command::PreparedDraft {
                request: 1,
                expected_edition: Some(expected),
                input: Box::new(input.clone()),
                prepared: vec![],
            },
        );
        assert!(result.unwrap_err().to_string().contains("Edition changed"));
        let issue = crate::magazine::load_issue(&c, input.id.as_ref().unwrap()).unwrap();
        assert_eq!(issue.title, "Newer title");
        assert!(
            issue.articles[0]
                .html
                .contains("Exact complete snapshot ending.")
        );
    }

    #[test]
    fn article_metadata_and_read_state_do_not_invalidate_frozen_draft() {
        let (c, input) = draft();
        let id = input.id.as_ref().unwrap();
        let expected = edition_fingerprint(&c, id).unwrap();
        c.execute("UPDATE articles SET title='Latest publisher title',excerpt='New preview',word_count=50 WHERE id=?1",[&input.article_ids[0].0]).unwrap();
        library::update(&c, &input.article_ids[0], None, Some(true)).unwrap();
        validate_prepared_edition(&c, &input, Some(&expected)).unwrap();
        assert_eq!(edition_fingerprint(&c, id).unwrap(), expected);
    }

    #[test]
    fn superseded_acquisition_cannot_publish_proposals_progress_or_failure() {
        let plans = crate::magazine::Plans {
            eligible_count: 1,
            classified_count: 0,
            proposals: vec![],
            unavailable: vec![],
        };
        for command in [
            Command::PreparedPlans { request: 7, plans },
            Command::AcquisitionProgress {
                request: 7,
                message: "Old progress".into(),
            },
            Command::AcquisitionFailed {
                request: 7,
                message: "Old failure".into(),
            },
        ] {
            assert!(!acquisition_is_current(&command, 8));
            assert!(acquisition_is_current(&command, 7));
        }
        let (_, input) = draft();
        let command = Command::PreparedDraft {
            request: 7,
            expected_edition: None,
            input: Box::new(input),
            prepared: vec![],
        };
        assert!(!acquisition_is_current(&command, 8));
    }
}
