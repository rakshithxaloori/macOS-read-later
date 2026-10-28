//! Publication icons share the disposable reader cache and its global budget.
use crate::{
    backend::Event,
    domain::{Article, CopyStatus},
    intake::Fetcher,
    network,
    storage::{CachedArticle, CachedMedia, Storage},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc::SyncSender,
    thread,
};

pub const MAX_ICON_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub struct Request {
    host: String,
    page: url::Url,
    source: Option<String>,
}
impl Request {
    pub fn from_article(article: &Article) -> Result<Self> {
        let page = network::normalize_url(&article.url)?;
        let host = page
            .host_str()
            .context("Publication has no hostname")?
            .to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
        Ok(Self {
            host,
            page,
            source: article.logo_url.clone(),
        })
    }
    pub fn key(&self) -> String {
        format!("publication-icon:{}", self.host)
    }
    fn urls(&self) -> Vec<String> {
        let mut urls = self
            .source
            .as_deref()
            .and_then(|s| network::normalize_url(s).ok())
            .map(|u| vec![u.to_string()])
            .unwrap_or_default();
        if let Ok(fallback) = self.page.join("/favicon.ico")
            && !urls.iter().any(|u| u == fallback.as_str())
        {
            urls.push(fallback.to_string());
        }
        urls
    }
}
pub fn requests(articles: &[Article]) -> Vec<Request> {
    let mut seen = HashSet::new();
    articles
        .iter()
        .filter_map(|article| Request::from_article(article).ok())
        .filter(|r| seen.insert(r.host.clone()))
        .collect()
}
pub fn identity(article: &Article) -> Option<String> {
    Request::from_article(article).ok().map(|r| r.key())
}

fn validate(bytes: &[u8]) -> Result<String> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_ICON_BYTES,
        "Publication icon exceeds 256 KiB"
    );
    let format = image::guess_format(bytes)?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Ico => "image/x-icon",
        _ => anyhow::bail!("Unsupported publication icon"),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()?;
    Ok(mime.into())
}
fn cached(storage: &Storage, key: &str) -> Result<Option<Vec<u8>>> {
    Ok(storage
        .cache_get(key)?
        .filter(|copy| copy.html.is_empty() && copy.media.len() == 1)
        .and_then(|mut copy| {
            let media = copy.media.remove(0);
            validate(&media.bytes).ok().map(|_| media.bytes)
        }))
}
pub fn cached_logos(
    connection: &rusqlite::Connection,
    articles: &[Article],
) -> Result<Vec<(String, Vec<u8>)>> {
    if crate::storage::database_path(connection)?.is_none() {
        return Ok(vec![]);
    }
    let storage = Storage::for_database(connection)?;
    let mut logos = Vec::new();
    for request in requests(articles) {
        let key = request.key();
        if let Some(bytes) = cached(&storage, &key)? {
            logos.push((key, bytes));
        }
    }
    Ok(logos)
}
pub async fn acquire(
    storage: &Storage,
    fetcher: &dyn Fetcher,
    request: &Request,
    offline: bool,
) -> Result<Option<Vec<u8>>> {
    let key = request.key();
    if let Some(bytes) = cached(storage, &key)? {
        return Ok(Some(bytes));
    }
    if offline {
        return Ok(None);
    }
    let epoch = storage.cache_generation(&key)?;
    for url in request.urls() {
        let Ok(resource) = fetcher.fetch(&url, true).await else {
            continue;
        };
        let Ok(mime_type) = validate(&resource.body) else {
            continue;
        };
        let bytes = resource.body;
        let copy = CachedArticle {
            html: String::new(),
            media: vec![CachedMedia {
                id: uuid::Uuid::new_v4().to_string(),
                mime_type,
                bytes: bytes.clone(),
            }],
            status: CopyStatus::Saved,
            error: None,
            word_count: 0,
            fetched_at: chrono::Utc::now().to_rfc3339(),
        };
        storage.cache_put(&key, epoch, false, copy)?;
        return Ok(Some(bytes));
    }
    Ok(None)
}

#[derive(Clone)]
struct Batch {
    generation: u64,
    requests: Vec<Request>,
}
pub struct Handle {
    commands: tokio::sync::watch::Sender<Option<Batch>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Handle {
    pub fn batch(&self, generation: u64, requests: Vec<Request>) {
        self.commands.send_replace(Some(Batch {
            generation,
            requests,
        }));
    }
    pub fn shutdown(&mut self) {
        self.commands.send_replace(None);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct PublicIcons;
#[async_trait::async_trait(?Send)]
impl Fetcher for PublicIcons {
    async fn fetch(&self, url: &str, _image: bool) -> Result<network::Resource> {
        network::fetch_icon(url).await
    }
}
pub fn spawn(
    path: PathBuf,
    offline: bool,
    events: SyncSender<Event>,
    ctx: egui::Context,
) -> Handle {
    // A single latest batch is bounded and cannot drop the newest generation
    // when rapid navigation outruns network completion.
    let (commands, mut receiver) = tokio::sync::watch::channel(None::<Batch>);
    let thread=thread::Builder::new().name("read-later-icons".into()).spawn(move||{
        let result=(||->Result<()>{
            let c=crate::db_compat::open_read_only(&path)?;
            if !crate::db_compat::is_lean(&c)? {return Ok(());}
            let storage=Storage::for_database(&c)?;
            let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            runtime.block_on(async {
                let mut generation=None;
                let mut pending=VecDeque::new();
                let mut seen=HashSet::new();
                loop {
                    let Some(request)=pending.pop_front() else {
                        if receiver.changed().await.is_err(){break;}
                        let Some(Batch{generation:next,requests})=receiver.borrow_and_update().clone() else {break;};
                        merge(next,requests,&mut generation,&mut pending,&mut seen);
                        continue;
                    };
                    let current=generation.expect("Batch supplied requests");
                    let work=acquire(&storage,&PublicIcons,&request,offline);tokio::pin!(work);
                    tokio::select! {
                        changed=receiver.changed()=>{
                            if changed.is_err(){break;}
                            let Some(Batch{generation:next,requests})=receiver.borrow_and_update().clone() else {break;};
                            if next==current {pending.push_front(request.clone());}
                            merge(next,requests,&mut generation,&mut pending,&mut seen);
                        },
                        result=&mut work=>if let Ok(Some(bytes))=result
                            && events.try_send(Event::Logos{generation:current,logos:vec![(request.key(),bytes)]}).is_ok(){ctx.request_repaint();}
                    }
                }
            });
            Ok(())
        })();
        // Icons are optional decoration. A missing icon keeps the publication's
        // letter placeholder and never changes article quality or reading state.
        let _=result;
    }).expect("Publication icon thread");
    Handle {
        commands,
        thread: Some(thread),
    }
}
fn merge(
    next: u64,
    requests: Vec<Request>,
    generation: &mut Option<u64>,
    pending: &mut VecDeque<Request>,
    seen: &mut HashSet<String>,
) {
    if *generation != Some(next) {
        pending.clear();
        seen.clear();
        *generation = Some(next);
    }
    for request in requests {
        if seen.insert(request.host.clone()) {
            pending.push_back(request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(12, 12)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }
    fn request(page: &str, source: Option<&str>) -> Request {
        let page = network::normalize_url(page).unwrap();
        let host = page
            .host_str()
            .unwrap()
            .strip_prefix("www.")
            .unwrap_or(page.host_str().unwrap())
            .to_owned();
        Request {
            host,
            page,
            source: source.map(str::to_owned),
        }
    }
    struct Fake {
        calls: RefCell<Vec<String>>,
        bytes: Vec<u8>,
    }
    #[async_trait::async_trait(?Send)]
    impl Fetcher for Fake {
        async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
            assert!(image);
            self.calls.borrow_mut().push(url.into());
            Ok(network::Resource {
                url: url.parse()?,
                body: self.bytes.clone(),
                content_type: "image/png".into(),
            })
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn private_retired_logo_uses_public_favicon_and_shared_cache() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::new(directory.path(), 0).unwrap();
        let fake = Fake {
            calls: RefCell::new(vec![]),
            bytes: png(),
        };
        let request = request(
            "https://www.example.org/article",
            Some("/api/media/retired"),
        );
        let first = acquire(&storage, &fake, &request, false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, fake.bytes);
        assert_eq!(
            &*fake.calls.borrow(),
            &["https://www.example.org/favicon.ico"]
        );
        assert_eq!(
            acquire(&storage, &fake, &request, true).await.unwrap(),
            Some(first)
        );
        assert_eq!(fake.calls.borrow().len(), 1);
        assert!(storage.snapshot().unwrap().cache_bytes > 0);
        let cached = storage.cache_get(&request.key()).unwrap().unwrap();
        assert!(cached.html.is_empty());
        assert_eq!(cached.media.len(), 1);
        storage.clear_cache().unwrap();
        assert!(
            acquire(&storage, &fake, &request, true)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test(flavor = "current_thread")]
    async fn icon_fetch_can_display_without_retaining_bytes_when_global_quota_is_full() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::new(directory.path(), 97_000_000).unwrap();
        let fake = Fake {
            calls: RefCell::new(vec![]),
            bytes: png(),
        };
        let request = request("https://example.org/article", None);
        assert_eq!(
            acquire(&storage, &fake, &request, false).await.unwrap(),
            Some(fake.bytes)
        );
        assert!(storage.cache_get(&request.key()).unwrap().is_none());
        assert_eq!(storage.snapshot().unwrap().cache_bytes, 0);
        assert!(storage.snapshot().unwrap().total_bytes() < 100_000_000);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn oversized_and_invalid_rasters_never_enter_cache() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::new(directory.path(), 0).unwrap();
        let mut oversized_dimensions = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2049, 1)
            .write_to(&mut oversized_dimensions, image::ImageFormat::Png)
            .unwrap();
        for bytes in [
            vec![0; MAX_ICON_BYTES + 1],
            b"not an icon".to_vec(),
            oversized_dimensions.into_inner(),
        ] {
            let fake = Fake {
                calls: RefCell::new(vec![]),
                bytes,
            };
            let request = request(
                "https://example.org/article",
                Some("http://127.0.0.1/private"),
            );
            assert!(
                acquire(&storage, &fake, &request, false)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(storage.cache_get(&request.key()).unwrap().is_none());
            assert_eq!(&*fake.calls.borrow(), &["https://example.org/favicon.ico"]);
        }
    }
    #[test]
    fn host_deduplication_and_new_generation_remove_old_pending_work() {
        let mut generation = None;
        let mut pending = VecDeque::new();
        let mut seen = HashSet::new();
        merge(
            1,
            vec![
                request("https://www.example.org/a", None),
                request("https://example.org/b", None),
            ],
            &mut generation,
            &mut pending,
            &mut seen,
        );
        assert_eq!(pending.len(), 1);
        merge(
            2,
            vec![request("https://other.org/c", None)],
            &mut generation,
            &mut pending,
            &mut seen,
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].key(), "publication-icon:other.org");
    }
}
