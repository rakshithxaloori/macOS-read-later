//! Foreground acquisition. Publisher bytes are disposable; library state is not.
use crate::{
    backend::Event,
    db_compat,
    domain::*,
    extraction,
    intake::Fetcher,
    library,
    storage::{CachedArticle, CachedMedia, Storage},
};
use anyhow::{Context, Result, ensure};
use scraper::{ElementRef, Html, Selector};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc::SyncSender,
    thread,
};

#[derive(Clone, Debug)]
pub struct Acquired {
    pub copy: extraction::Extraction,
    pub media: Vec<CachedMedia>,
    pub fetched_at: String,
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn image_sources(html: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    Html::parse_fragment(html)
        .select(&Selector::parse("img").expect("static selector"))
        .map(|node| node.value().attr("src").unwrap_or_default().to_owned())
        .filter(|source| seen.insert(source.clone()))
        .collect()
}
/// Serialize the sanitized DOM while replacing actual image nodes. Matching
/// serialized substrings is unsafe: HTML void tags and quote styles differ
/// between extraction and scraper's serializer.
fn rewrite_images(html: &str, mut replace: impl FnMut(&str, &str) -> String) -> String {
    fn children(
        element: ElementRef<'_>,
        replace: &mut impl FnMut(&str, &str) -> String,
        output: &mut String,
    ) {
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                output.push_str(&escape(text));
            } else if let Some(element) = ElementRef::wrap(child) {
                if element.value().name() == "img" {
                    output.push_str(&replace(
                        element.value().attr("src").unwrap_or_default(),
                        element.value().attr("alt").unwrap_or_default(),
                    ));
                    continue;
                }
                let tag = element.value().name();
                output.push('<');
                output.push_str(tag);
                for (name, value) in element.value().attrs() {
                    output.push(' ');
                    output.push_str(name);
                    output.push_str("=\"");
                    output.push_str(&escape(value));
                    output.push('"');
                }
                output.push('>');
                if !matches!(
                    tag,
                    "area"
                        | "base"
                        | "br"
                        | "col"
                        | "embed"
                        | "hr"
                        | "input"
                        | "link"
                        | "meta"
                        | "param"
                        | "source"
                        | "track"
                        | "wbr"
                ) {
                    children(element, replace, output);
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
            }
        }
    }
    let document = Html::parse_fragment(html);
    let mut output = String::new();
    children(document.root_element(), &mut replace, &mut output);
    output
}
fn image_mime(bytes: &[u8]) -> Result<String> {
    let format = image::guess_format(bytes)?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::Ico => "image/x-icon",
        _ => anyhow::bail!("Unsupported raster image"),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(100 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()?;
    Ok(mime.into())
}
/// Deliver safe prose before images. The fetcher is replaceable by synthetic data.
pub async fn acquire(
    fetcher: &dyn Fetcher,
    url: &str,
    on_text: impl Fn(&extraction::Extraction),
) -> Result<Acquired> {
    let resource = fetcher.fetch(url, false).await?;
    let source =
        std::str::from_utf8(&resource.body).context("Publisher page must use UTF-8 encoding")?;
    let mut copy =
        extraction::extract(source, resource.url.as_str()).map_err(anyhow::Error::msg)?;
    ensure!(
        copy.word_count > 0,
        "Publisher did not provide a readable article"
    );
    let images = image_sources(&copy.html);
    let mut prose = copy.clone();
    prose.html = rewrite_images(&copy.html, |_, alt| {
        format!(
            "<span>[Loading image{}]</span>",
            if alt.is_empty() {
                String::new()
            } else {
                format!(": {}", escape(alt))
            }
        )
    });
    on_text(&prose);
    let mut media = Vec::new();
    let mut paths = HashMap::new();
    let mut bytes = 0usize;
    let mut missing = 0usize;
    // Two downloads overlap, while all parsing and decoded bytes remain bounded.
    for pair in images.chunks(2).take(11) {
        let fetch = |source: String| async move { fetcher.fetch(&source, true).await };
        let first = fetch(pair[0].clone());
        let (one, two) = if pair.len() == 2 {
            let (a, b) = tokio::join!(first, fetch(pair[1].clone()));
            (a, Some(b))
        } else {
            (first.await, None)
        };
        for (source, response) in pair.iter().zip(std::iter::once(one).chain(two)) {
            let decoded = response.and_then(|r| {
                ensure!(
                    bytes + r.body.len() <= 12_000_000,
                    "Article images exceed the reader working limit"
                );
                let mime = image_mime(&r.body)?;
                Ok((r.body, mime))
            });
            if let Ok((body, mime_type)) = decoded {
                bytes += body.len();
                let id = uuid::Uuid::new_v4().to_string();
                paths.insert(source.clone(), id.clone());
                media.push(CachedMedia {
                    id,
                    mime_type,
                    bytes: body,
                });
            }
        }
    }
    copy.html = rewrite_images(&copy.html, |source, alt| {
        if let Some(id) = paths.get(source) {
            format!("<img src=\"/api/media/{id}\" alt=\"{}\">", escape(alt))
        } else {
            missing += 1;
            format!(
                "<a href=\"{}\">[Image unavailable{}. View original]</a>",
                escape(url),
                if alt.is_empty() {
                    String::new()
                } else {
                    format!(": {}", escape(alt))
                }
            )
        }
    });
    copy.html = extraction::clean_html(&copy.html, resource.url.as_str(), false);
    if missing > 0 {
        copy.status = CopyStatus::Partial;
        let notice = format!("{missing} image(s) could not be loaded");
        copy.error = Some(match copy.error {
            Some(previous) => format!("{previous}; {notice}"),
            None => notice,
        });
    }
    Ok(Acquired {
        copy,
        media,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[derive(Debug)]
pub enum Command {
    Open {
        generation: u64,
        id: ArticleId,
        refresh: bool,
    },
    Cancel,
    Quit,
}
pub struct Handle {
    pub commands: tokio::sync::mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Handle {
    pub fn shutdown(&mut self) {
        let _ = self.commands.blocking_send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn emit(tx: &SyncSender<Event>, ctx: &egui::Context, event: Event) {
    if tx.send(event).is_ok() {
        ctx.request_repaint();
    }
}
fn body(mut article: Article, copy: CachedArticle) -> ArticleBody {
    article.word_count = copy.word_count;
    article.status = copy.status;
    article.error = copy.error.clone();
    ArticleBody {
        article,
        html: copy.html,
        media: copy.media.into_iter().map(|m| (m.id, m.bytes)).collect(),
    }
}
struct LoadContext<'a> {
    c: &'a rusqlite::Connection,
    offline: bool,
    tx: &'a SyncSender<Event>,
    ctx: &'a egui::Context,
    fetcher: &'a dyn Fetcher,
}
impl LoadContext<'_> {
    async fn load(&self, generation: &Cell<u64>, id: ArticleId, refresh: bool) -> Result<()> {
        let Self {
            c,
            offline,
            tx,
            ctx,
            fetcher,
        } = *self;
        let mut article = library::metadata(c, &id)?;
        if !db_compat::is_lean(c)? {
            emit(
                tx,
                ctx,
                Event::Body {
                    generation: generation.get(),
                    body: Box::new(library::body(c, &id)?),
                    fetched_at: None,
                },
            );
            return Ok(());
        }
        let storage = Storage::for_database(c)?;
        let epoch = storage.cache_generation(&id.0)?;
        let retained = if article.archived_at.is_none() {
            storage.cache_get(&id.0)?
        } else {
            None
        };
        if let Some(copy) = &retained {
            let fetched_at = Some(copy.fetched_at.clone());
            emit(
                tx,
                ctx,
                Event::Body {
                    generation: generation.get(),
                    body: Box::new(body(article.clone(), copy.clone())),
                    fetched_at,
                },
            );
            if !refresh {
                return Ok(());
            }
        }
        ensure!(
            !offline,
            if retained.is_some() {
                "Connect to the internet to refresh. The existing cached copy was retained."
            } else {
                "Article is not cached. Connect to the internet and retry."
            }
        );
        let acquired = acquire(fetcher, &article.url, |copy| {
            // A valid full cached document remains readable while refreshing.
            // Do not briefly replace it with potentially weaker incoming prose.
            if retained.is_some() {
                return;
            }
            let Ok(mut preview) = library::metadata(c, &id) else {
                return;
            };
            apply_metadata(&mut preview, copy);
            emit(
                tx,
                ctx,
                Event::Body {
                    generation: generation.get(),
                    body: Box::new(ArticleBody {
                        article: preview,
                        html: copy.html.clone(),
                        media: vec![],
                    }),
                    fetched_at: Some(chrono::Utc::now().to_rfc3339()),
                },
            );
        })
        .await;
        let acquired = match acquired {
            Ok(acquired) => acquired,
            Err(error) if retained.is_some() => {
                return Err(error.context("Refresh failed; the existing cached copy was retained"));
            }
            Err(error) => return Err(error),
        };
        // Metadata is reread after acquisition so archive/read actions never regress.
        article = library::metadata(c, &id)?;
        if let Some(copy) = retained
            && let extraction::RetentionDecision::Keep { diagnostic, .. } =
                extraction::retention_decision(&copy.html, copy.status, &acquired.copy)
        {
            let detail = acquired
                .copy
                .error
                .as_ref()
                .map(|error| format!(" {error}"))
                .unwrap_or_default();
            emit(
                tx,
                ctx,
                Event::Body {
                    generation: generation.get(),
                    fetched_at: Some(copy.fetched_at.clone()),
                    body: Box::new(body(article, copy)),
                },
            );
            emit(
                tx,
                ctx,
                Event::BodyError {
                    generation: generation.get(),
                    id,
                    message: format!("{diagnostic}{detail}"),
                },
            );
            return Ok(());
        }
        apply_metadata(&mut article, &acquired.copy);
        let copy = CachedArticle {
            html: acquired.copy.html,
            media: acquired.media,
            status: acquired.copy.status,
            error: acquired.copy.error,
            word_count: acquired.copy.word_count,
            fetched_at: acquired.fetched_at,
        };
        // Quota rejection leaves the current in-memory view usable. Filesystem or
        // policy errors must remain visible instead of pretending caching worked.
        storage.cache_put(&id.0, epoch, article.archived_at.is_some(), copy.clone())?;
        emit(
            tx,
            ctx,
            Event::Body {
                generation: generation.get(),
                fetched_at: Some(copy.fetched_at.clone()),
                body: Box::new(body(article, copy)),
            },
        );
        Ok(())
    }

    async fn serve(&self, rx: &mut tokio::sync::mpsc::Receiver<Command>) {
        let mut pending = None;
        loop {
            let command = match pending.take() {
                Some(command) => Some(command),
                None => rx.recv().await,
            };
            let Some(Command::Open {
                generation,
                id,
                refresh,
            }) = command
            else {
                if matches!(command, Some(Command::Quit) | None) {
                    break;
                }
                continue;
            };
            let generation = Cell::new(generation);
            let work = self.load(&generation, id.clone(), refresh);
            tokio::pin!(work);
            loop {
                tokio::select! {
                    // Queued navigation wins over a simultaneously ready fetch.
                    biased;
                    next = rx.recv() => {
                        if let Some(Command::Open { generation: newer, id: next_id, refresh: false }) = &next
                            && next_id == &id {
                            generation.set(*newer);
                            continue;
                        }
                        pending = next;
                        break;
                    },
                    result = &mut work => {
                        if let Err(error) = result {
                            emit(self.tx, self.ctx, Event::BodyError {
                                generation: generation.get(), id, message: format!("{error:#}")
                            });
                        }
                        break;
                    }
                }
            }
        }
    }
}
fn apply_metadata(article: &mut Article, copy: &extraction::Extraction) {
    if !copy.title.trim().is_empty() {
        article.title = copy.title.chars().take(1000).collect();
    }
    if let Some(author) = &copy.author {
        article.author = Some(author.chars().take(500).collect());
    }
    if !copy.site.trim().is_empty() {
        article.site = copy.site.chars().take(500).collect();
    }
    if !copy.excerpt.trim().is_empty() {
        article.excerpt = copy.excerpt.chars().take(260).collect();
    }
    article.word_count = copy.word_count;
    article.status = copy.status;
    article.error = copy.error.as_ref().map(|s| s.chars().take(2000).collect());
    if copy.published_at.is_some() {
        article.published_at = copy.published_at.clone();
    }
}
pub fn spawn(path: PathBuf, offline: bool, tx: SyncSender<Event>, ctx: egui::Context) -> Handle {
    let (commands, mut rx) = tokio::sync::mpsc::channel(16);
    let thread = thread::Builder::new()
        .name("read-later-reader".into())
        .spawn(move || {
            let result = (|| -> Result<()> {
                let c = db_compat::open_read_only(&path)?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let context = LoadContext {
                    c: &c,
                    offline,
                    tx: &tx,
                    ctx: &ctx,
                    fetcher: &crate::intake::PublicFetcher,
                };
                runtime.block_on(context.serve(&mut rx));
                Ok(())
            })();
            if let Err(error) = result {
                emit(
                    &tx,
                    &ctx,
                    Event::Error(format!("Reader loader failed: {error:#}")),
                );
            }
        })
        .expect("Reader background thread");
    Handle {
        commands,
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network;
    use std::cell::Cell;
    #[test]
    fn image_rewriting_handles_void_tags_quotes_and_keeps_native_structure() {
        let html = "<figure><img alt='A &amp; B' src=https://example.com/a.png/><figcaption>Caption &amp; credit</figcaption></figure><p id='note'>Footnote <a href='#note'>back</a><br/>end</p><table><tr><td colspan='2'>Cell</td></tr></table><img src=\"https://example.com/b.png\" alt=\"Second\">";
        let mut replaced = Vec::new();
        let prose = rewrite_images(html, |source, alt| {
            replaced.push((source.to_owned(), alt.to_owned()));
            format!("<span>{}</span>", escape(alt))
        });
        assert_eq!(replaced.len(), 2);
        assert!(!prose.contains("<img"));
        assert!(prose.contains("A &amp; B"));
        assert!(prose.contains("<figcaption>Caption &amp; credit</figcaption>"));
        assert!(prose.contains("id=\"note\""));
        assert!(prose.contains("href=\"#note\""));
        assert!(prose.contains("colspan=\"2\""));
    }

    fn cached_fixture() -> (
        tempfile::TempDir,
        rusqlite::Connection,
        ArticleId,
        CachedArticle,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = db_compat::open(&directory.path().join("library.sqlite3")).unwrap();
        db_compat::initialize_lean(&connection).unwrap();
        let (id, _) = library::save(
            &mut connection,
            "https://example.com/article",
            ArrivalKind::Web,
            "cached-refresh",
            "Saved",
            Some("Existing useful metadata"),
            None,
        )
        .unwrap();
        let html = format!(
            "<h2>Preserved heading</h2><p>{}</p>",
            "Complete original observations remain available to the reader. ".repeat(500)
        );
        let copy = CachedArticle {
            word_count: extraction::word_count(&html),
            html,
            media: Vec::new(),
            status: CopyStatus::Saved,
            error: None,
            fetched_at: "2026-10-09T12:00:00Z".into(),
        };
        let storage = Storage::for_database(&connection).unwrap();
        let epoch = storage.cache_generation(&id.0).unwrap();
        assert!(
            storage
                .cache_put(&id.0, epoch, false, copy.clone())
                .unwrap()
        );
        (directory, connection, id, copy)
    }

    struct Failed;
    #[async_trait::async_trait(?Send)]
    impl Fetcher for Failed {
        async fn fetch(&self, _: &str, _: bool) -> Result<network::Resource> {
            anyhow::bail!("Synthetic publisher failure")
        }
    }

    #[tokio::test]
    async fn failed_refresh_keeps_cached_content_time_quality_and_metadata() {
        let (_directory, c, id, retained) = cached_fixture();
        let before = serde_json::to_value(library::metadata(&c, &id).unwrap()).unwrap();
        let storage = Storage::for_database(&c).unwrap();
        let epoch = storage.cache_generation(&id.0).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        let error = LoadContext {
            c: &c,
            offline: false,
            tx: &tx,
            ctx: &ctx,
            fetcher: &Failed,
        }
        .load(&Cell::new(2), id.clone(), true)
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("publisher failure"));
        assert!(error.to_string().contains("cached copy was retained"));
        let events = rx.try_iter().collect::<Vec<_>>();
        assert!(
            matches!(&events[..], [Event::Body { body, fetched_at: Some(at), .. }] if body.html==retained.html && body.article.status==CopyStatus::Saved && body.article.error.is_none() && at==&retained.fetched_at)
        );
        let after = storage.cache_get(&id.0).unwrap().unwrap();
        assert_eq!(after.html, retained.html);
        assert_eq!(after.fetched_at, retained.fetched_at);
        assert_eq!(after.status, CopyStatus::Saved);
        assert!(after.error.is_none());
        assert_eq!(storage.cache_generation(&id.0).unwrap(), epoch);
        assert_eq!(
            serde_json::to_value(library::metadata(&c, &id).unwrap()).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn weaker_refresh_keeps_richer_copy_with_a_separate_attempt_diagnostic() {
        let (_directory, c, id, retained) = cached_fixture();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        let fetcher = Fake {
            images: Cell::new(0),
            valid_image: false,
        };
        LoadContext {
            c: &c,
            offline: false,
            tx: &tx,
            ctx: &ctx,
            fetcher: &fetcher,
        }
        .load(&Cell::new(3), id.clone(), true)
        .await
        .unwrap();
        let events = rx.try_iter().collect::<Vec<_>>();
        assert!(
            matches!(&events.last(), Some(Event::BodyError { generation: 3, message, .. }) if message.contains("shorter") && message.contains("image(s) could not be loaded"))
        );
        for event in &events {
            if let Event::Body {
                body, fetched_at, ..
            } = event
            {
                assert_eq!(body.html, retained.html);
                assert_eq!(body.article.status, CopyStatus::Saved);
                assert!(body.article.error.is_none());
                assert_eq!(fetched_at.as_ref(), Some(&retained.fetched_at));
            }
        }
        let cached = Storage::for_database(&c)
            .unwrap()
            .cache_get(&id.0)
            .unwrap()
            .unwrap();
        assert_eq!(cached.html, retained.html);
        assert_eq!(cached.fetched_at, retained.fetched_at);
        assert!(cached.error.is_none());
    }

    struct PageWait<'a> {
        cancelled: &'a Cell<bool>,
        completed: bool,
    }
    impl Drop for PageWait<'_> {
        fn drop(&mut self) {
            if !self.completed {
                self.cancelled.set(true);
            }
        }
    }
    struct Gated {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
        cancelled: Cell<bool>,
        pages: Cell<usize>,
    }
    impl Gated {
        fn new() -> Self {
            Self {
                started: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
                cancelled: Cell::new(false),
                pages: Cell::new(0),
            }
        }
    }
    #[async_trait::async_trait(?Send)]
    impl Fetcher for Gated {
        async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
            if !image && url == "https://example.com/article" {
                self.pages.set(self.pages.get() + 1);
                let mut wait = PageWait {
                    cancelled: &self.cancelled,
                    completed: false,
                };
                self.started.notify_one();
                self.release.notified().await;
                wait.completed = true;
            }
            Fake {
                images: Cell::new(0),
                valid_image: true,
            }
            .fetch(url, image)
            .await
        }
    }

    async fn wait_for_cache(storage: &Storage, id: &ArticleId) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while storage.cache_get(&id.0).unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("foreground actor did not publish its selected article");
    }

    #[tokio::test]
    async fn newer_navigation_cancels_old_fetch_without_cache_or_stale_results() {
        let (_directory, mut c, first, _retained) = cached_fixture();
        let storage = Storage::for_database(&c).unwrap();
        storage.invalidate(&first.0).unwrap();
        let (second, _) = library::save(
            &mut c,
            "https://example.com/second",
            ArrivalKind::Web,
            "second",
            "Saved",
            None,
            None,
        )
        .unwrap();
        let (events_tx, events_rx) = std::sync::mpsc::sync_channel(8);
        let (commands, mut requests) = tokio::sync::mpsc::channel(8);
        let ctx = egui::Context::default();
        let fetcher = Gated::new();
        let context = LoadContext {
            c: &c,
            offline: false,
            tx: &events_tx,
            ctx: &ctx,
            fetcher: &fetcher,
        };
        commands
            .send(Command::Open {
                generation: 1,
                id: first.clone(),
                refresh: false,
            })
            .await
            .unwrap();
        let navigate = async {
            fetcher.started.notified().await;
            commands
                .send(Command::Open {
                    generation: 2,
                    id: second.clone(),
                    refresh: false,
                })
                .await
                .unwrap();
            wait_for_cache(&storage, &second).await;
            commands.send(Command::Quit).await.unwrap();
        };
        tokio::join!(context.serve(&mut requests), navigate);
        assert!(fetcher.cancelled.get());
        assert!(storage.cache_get(&first.0).unwrap().is_none());
        let events = events_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| matches!(event, Event::Body {generation: 2, body, ..} if body.article.id==second)));
    }

    #[tokio::test]
    async fn same_article_requests_share_a_fetch_and_use_the_newest_generation() {
        let (_directory, c, id, _retained) = cached_fixture();
        let storage = Storage::for_database(&c).unwrap();
        storage.invalidate(&id.0).unwrap();
        let (events_tx, events_rx) = std::sync::mpsc::sync_channel(8);
        let (commands, mut requests) = tokio::sync::mpsc::channel(8);
        let ctx = egui::Context::default();
        let fetcher = Gated::new();
        let context = LoadContext {
            c: &c,
            offline: false,
            tx: &events_tx,
            ctx: &ctx,
            fetcher: &fetcher,
        };
        commands
            .send(Command::Open {
                generation: 1,
                id: id.clone(),
                refresh: false,
            })
            .await
            .unwrap();
        let reselect = async {
            fetcher.started.notified().await;
            commands
                .send(Command::Open {
                    generation: 2,
                    id: id.clone(),
                    refresh: false,
                })
                .await
                .unwrap();
            fetcher.release.notify_one();
            wait_for_cache(&storage, &id).await;
            commands.send(Command::Quit).await.unwrap();
        };
        tokio::join!(context.serve(&mut requests), reselect);
        assert_eq!(fetcher.pages.get(), 1);
        assert!(!fetcher.cancelled.get());
        let events = events_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(
            |event| matches!(event, Event::Body {generation: 2, body, ..} if body.article.id==id)
        ));
    }

    #[tokio::test]
    async fn failed_refresh_after_archive_never_restores_purged_cached_content() {
        let (directory, c, id, _retained) = cached_fixture();
        struct ArchiveAndFail<'a> {
            path: &'a std::path::Path,
            id: &'a ArticleId,
        }
        #[async_trait::async_trait(?Send)]
        impl Fetcher for ArchiveAndFail<'_> {
            async fn fetch(&self, _: &str, _: bool) -> Result<network::Resource> {
                let writer = db_compat::open(self.path)?;
                library::update(&writer, self.id, None, Some(true))?;
                anyhow::bail!("Publisher is unavailable after archive")
            }
        }
        let path = directory.path().join("library.sqlite3");
        let fetcher = ArchiveAndFail {
            path: &path,
            id: &id,
        };
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        assert!(
            LoadContext {
                c: &c,
                offline: false,
                tx: &tx,
                ctx: &ctx,
                fetcher: &fetcher
            }
            .load(&Cell::new(3), id.clone(), true)
            .await
            .is_err()
        );
        assert!(
            Storage::for_database(&c)
                .unwrap()
                .cache_get(&id.0)
                .unwrap()
                .is_none()
        );
        let article = library::metadata(&c, &id).unwrap();
        assert!(article.archived_at.is_some());
        assert!(article.read_at.is_some());
    }
    struct Fake {
        images: Cell<usize>,
        valid_image: bool,
    }
    #[async_trait::async_trait(?Send)]
    impl Fetcher for Fake {
        async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
            let body = if image {
                self.images.set(self.images.get() + 1);
                if self.valid_image {
                    let mut out = std::io::Cursor::new(Vec::new());
                    image::DynamicImage::new_rgb8(2, 2)
                        .write_to(&mut out, image::ImageFormat::Png)?;
                    out.into_inner()
                } else {
                    b"not an image".to_vec()
                }
            } else {
                format!("<html><head><title>Synthetic science article</title></head><body><article><h1>Synthetic science article</h1><p>{}</p><table><tr><td>First value</td><td>Second value</td></tr></table><figure><img src=\"https://example.com/image.png\" alt=\"sample\"><figcaption>A sample caption</figcaption></figure><p id=\"note\">A footnote.</p></article></body></html>","Experiments describe measurable results and preserve exact observations for future scientific comparison. ".repeat(70)).into_bytes()
            };
            Ok(network::Resource {
                url: url::Url::parse(url)?,
                body,
                content_type: if image { "image/png" } else { "text/html" }.into(),
            })
        }
    }
    #[tokio::test]
    async fn prose_precedes_images_and_native_structures_survive() {
        let fake = Fake {
            images: Cell::new(0),
            valid_image: true,
        };
        let delivered = Cell::new(false);
        let copy = acquire(&fake, "https://example.com/article", |prose| {
            assert_eq!(fake.images.get(), 0);
            assert!(!prose.html.contains("src=\"https://"));
            assert!(prose.html.contains("<table>"));
            delivered.set(true);
        })
        .await
        .unwrap();
        assert!(delivered.get());
        assert_eq!(copy.media.len(), 1);
        assert!(copy.copy.html.contains("/api/media/"));
        assert!(copy.copy.html.contains("<figcaption>"));
        assert!(copy.copy.html.contains("id=\"note\""));
        assert!(copy.copy.word_count > 500);
    }
    #[tokio::test]
    async fn unavailable_image_is_explicit_and_never_a_fabricated_attachment() {
        let copy = acquire(
            &Fake {
                images: Cell::new(0),
                valid_image: false,
            },
            "https://example.com/article",
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(copy.copy.status, CopyStatus::Partial);
        assert!(copy.media.is_empty());
        assert!(copy.copy.html.contains("Image unavailable"));
        assert!(!copy.copy.html.contains("<img"));
    }

    #[tokio::test]
    async fn archive_during_image_fetch_preserves_state_and_fences_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite3");
        let mut writer = db_compat::open(&path).unwrap();
        db_compat::initialize_lean(&writer).unwrap();
        let (id, _) = library::save(
            &mut writer,
            "https://example.com/article",
            ArrivalKind::Web,
            "race",
            "Saved",
            Some("Existing title"),
            None,
        )
        .unwrap();
        let c = db_compat::open_read_only(&path).unwrap();
        let generation = Cell::new(7);
        struct Archiving<'a> {
            path: &'a std::path::Path,
            id: &'a ArticleId,
            generation: &'a Cell<u64>,
        }
        #[async_trait::async_trait(?Send)]
        impl Fetcher for Archiving<'_> {
            async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
                if image {
                    let c = db_compat::open(self.path)?;
                    library::update(&c, self.id, None, Some(true))?;
                    self.generation.set(8);
                }
                Fake {
                    images: Cell::new(0),
                    valid_image: true,
                }
                .fetch(url, image)
                .await
            }
        }
        let fetcher = Archiving {
            path: &path,
            id: &id,
            generation: &generation,
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        LoadContext {
            c: &c,
            offline: false,
            tx: &tx,
            ctx: &ctx,
            fetcher: &fetcher,
        }
        .load(&generation, id.clone(), false)
        .await
        .unwrap();
        let events = rx.try_iter().collect::<Vec<_>>();
        assert!(
            matches!(&events[0],Event::Body {generation:7,body,..} if body.media.is_empty() && body.article.read_at.is_none())
        );
        assert!(
            matches!(&events[1],Event::Body {generation:8,body,..} if body.media.len()==1 && body.article.archived_at.is_some() && body.article.read_at.is_some())
        );
        assert!(
            Storage::for_database(&c)
                .unwrap()
                .cache_get(&id.0)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            writer
                .query_row("SELECT html FROM articles WHERE id=?1", [&id.0], |r| r
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            ""
        );
    }

    #[tokio::test]
    async fn expired_cache_and_failed_load_leave_metadata_and_reading_state_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite3");
        let mut c = db_compat::open(&path).unwrap();
        db_compat::initialize_lean(&c).unwrap();
        let (id, _) = library::save(
            &mut c,
            "https://example.com/article",
            ArrivalKind::Web,
            "offline",
            "Saved",
            Some("Preserved title"),
            None,
        )
        .unwrap();
        library::update(&c, &id, Some(true), None).unwrap();
        let before = serde_json::to_value(library::metadata(&c, &id).unwrap()).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        let fake = Fake {
            images: Cell::new(0),
            valid_image: true,
        };
        let error = LoadContext {
            c: &c,
            offline: true,
            tx: &tx,
            ctx: &ctx,
            fetcher: &fake,
        }
        .load(&Cell::new(9), id.clone(), false)
        .await
        .unwrap_err();
        assert!(error.to_string().contains("not cached"));
        assert!(rx.try_iter().next().is_none());
        assert_eq!(
            serde_json::to_value(library::metadata(&c, &id).unwrap()).unwrap(),
            before
        );
    }
}
