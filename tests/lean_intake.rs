use anyhow::Result;
use macos_read_later::{
    db_compat,
    domain::*,
    exports, imports,
    intake::{self, Fetcher},
    library, network,
};
use rusqlite::{Connection, params};
use serde_json::json;
use std::cell::Cell;

fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    db_compat::register(&c).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    db_compat::initialize_lean(&c).unwrap();
    intake::initialize(&c).unwrap();
    c
}
fn xml() -> String {
    "<rss version='2.0'><channel><title>News</title><link>https://example.org</link><description>News</description><item><title>News article</title><link>https://example.org/a</link><description><![CDATA[<p>Short durable preview.</p><img src='https://example.org/a.png'>]]></description></item></channel></rss>".into()
}
struct Unavailable;
#[async_trait::async_trait(?Send)]
impl Fetcher for Unavailable {
    async fn fetch(&self, _: &str, _: bool) -> Result<network::Resource> {
        anyhow::bail!("No network during replay")
    }
}
#[tokio::test(flavor = "current_thread")]
async fn lean_feed_crash_replay_reclaims_xml_without_publisher_jobs_or_resurrection() {
    let mut c = db();
    let feed = library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    intake::persist_capture(&mut c, &lease, &job, &xml()).unwrap();
    intake::release_worker(&mut c, &lease).unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    assert!(intake::run_one(&mut c, &lease, &Unavailable).await.unwrap());
    assert_eq!(
        c.query_row("SELECT count(*) FROM feed_captures", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM jobs WHERE kind='article'", [], |r| {
            r.get::<_, u32>(0)
        })
        .unwrap(),
        0
    );
    let a = library::articles(&c, &Query::for_scope(Scope::Inbox), 0)
        .unwrap()
        .remove(0);
    assert_eq!(a.excerpt, "Short durable preview.");
    assert_eq!(a.word_count, 0);
    assert!(library::body(&c, &a.id).unwrap().html.is_empty());
    library::update(&c, &a.id, None, Some(true)).unwrap();
    library::delete(&mut c, std::slice::from_ref(&a.id)).unwrap();
    library::enqueue(&c, "feed", &feed.0).unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    let captured = intake::persist_capture(&mut c, &lease, &job, &xml()).unwrap();
    intake::replay_capture(&mut c, &lease, &job, &captured).unwrap();
    assert!(library::metadata(&c, &a.id).is_err());
    assert_eq!(
        c.query_row("SELECT count(*) FROM sources", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM native_feed_state", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn file_backed_rss_pending_buffer_is_shared_and_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let mut c = db_compat::open(&directory.path().join("library.sqlite3")).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    intake::initialize(&c).unwrap();
    library::add_feed(&mut c, "https://example.org/feed").unwrap();
    library::add_feed(&mut c, "https://other.example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    // Capture persistence precedes parsing, so raw invalid XML is also bounded.
    let half = network::MAX_RESPONSE / 2;
    intake::persist_capture(&mut c, &lease, &job, &"x".repeat(half)).unwrap();
    intake::finish_job(&mut c, &lease, &job, Ok(None)).unwrap();
    let next = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    assert_ne!(next.target_id, job.target_id);
    let job = next;
    intake::persist_capture(&mut c, &lease, &job, &"y".repeat(half)).unwrap();
    let error = intake::persist_capture(&mut c, &lease, &job, "z").unwrap_err();
    assert!(error.to_string().contains("RSS replay buffer is full"));
    assert_eq!(c.query_row("SELECT count(*),sum(length(CAST(xml AS BLOB))) FROM feed_captures WHERE processed_at IS NULL",[],|r|Ok((r.get::<_,u32>(0)?,r.get::<_,i64>(1)?))).unwrap(),(2,network::MAX_RESPONSE as i64));
}

#[test]
fn a_library_past_the_old_100_mb_limit_keeps_saving_importing_and_capturing() {
    use macos_read_later::storage::Storage;
    let directory = tempfile::tempdir().unwrap();
    let mut c = db_compat::open(&directory.path().join("library.sqlite3")).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    intake::initialize(&c).unwrap();
    let storage = Storage::for_database(&c).unwrap();
    // A 45 MB app with 120 MB of library work in flight: these refusals were
    // "Storage budget exhausted" before the library cap was removed.
    storage.set_bundle_bytes(45_000_000).unwrap();
    let _work = storage.reserve("large import", 120_000_000).unwrap();
    let usage = storage.snapshot().unwrap();
    assert!(usage.total_bytes() + usage.reserved_bytes > 100_000_000);

    library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    let capture = intake::persist_capture(&mut c, &lease, &job, &xml()).unwrap();
    intake::replay_capture(&mut c, &lease, &job, &capture).unwrap();

    library::save(
        &mut c,
        "https://example.org/saved",
        ArrivalKind::Web,
        "over-limit-save",
        "Manual save",
        Some("Saved past the old limit"),
        None,
    )
    .unwrap();

    let upload = tempfile::tempdir().unwrap();
    let csv = upload.path().join("rows.csv");
    let mut rows = "url,title\n".to_owned();
    for i in 0..30 {
        rows.push_str(&format!("https://example.org/{i},{}\n", "x".repeat(900)));
    }
    std::fs::write(&csv, rows).unwrap();
    let run = imports::stage(&mut c, &csv).unwrap();
    imports::start(&mut c, &run, false).unwrap();
    while imports::run_batch(&mut c).unwrap() {}
    assert_eq!(imports::list(&c).unwrap().remove(0).state, "completed");

    assert!(library::counts(&c).unwrap().all >= 32);
}

#[test]
fn headline_only_feeds_keep_author_without_claiming_complete_content() {
    let mut c = db();
    library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    let xml = "<feed xmlns='http://www.w3.org/2005/Atom'><title>Headlines</title><id>https://example.org/feed</id><updated>2026-10-14T00:00:00Z</updated><entry><title>Headline only</title><id>https://example.org/headline</id><updated>2026-10-14T00:00:00Z</updated><link href='https://example.org/headline'/><author><name>Known author</name></author></entry></feed>";
    let capture = intake::persist_capture(&mut c, &lease, &job, xml).unwrap();
    intake::replay_capture(&mut c, &lease, &job, &capture).unwrap();
    let article = library::articles(&c, &Query::for_scope(Scope::Inbox), 0)
        .unwrap()
        .remove(0);
    assert_eq!(article.author.as_deref(), Some("Known author"));
    assert_eq!(article.status, CopyStatus::Pending);
    assert_eq!(article.word_count, 0);
    assert!(article.excerpt.is_empty());
    assert!(library::body(&c, &article.id).unwrap().html.is_empty());
}
struct Conditional {
    calls: Cell<u32>,
}
#[async_trait::async_trait(?Send)]
impl Fetcher for Conditional {
    async fn fetch(&self, _: &str, _: bool) -> Result<network::Resource> {
        anyhow::bail!("Use conditional feed adapter")
    }
    async fn fetch_feed(
        &self,
        _: &str,
        etag: Option<&str>,
        _: Option<&str>,
    ) -> Result<network::ConditionalResource> {
        self.calls.set(self.calls.get() + 1);
        if self.calls.get() == 1 {
            assert!(etag.is_none());
            Ok(network::ConditionalResource {
                resource: Some(network::Resource {
                    url: "https://example.org/feed".parse().unwrap(),
                    body: xml().into_bytes(),
                    content_type: "application/rss+xml".into(),
                }),
                etag: Some("v1".into()),
                last_modified: None,
            })
        } else {
            assert_eq!(etag, Some("v1"));
            Ok(network::ConditionalResource {
                resource: None,
                etag: Some("v1".into()),
                last_modified: None,
            })
        }
    }
}
#[tokio::test(flavor = "current_thread")]
async fn validators_survive_xml_retirement_and_304_keeps_state() {
    let mut c = db();
    let feed = library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let f = Conditional {
        calls: Cell::new(0),
    };
    intake::run_one(&mut c, &lease, &f).await.unwrap();
    let id = library::articles(&c, &Query::for_scope(Scope::Inbox), 0).unwrap()[0]
        .id
        .clone();
    library::update(&c, &id, None, Some(true)).unwrap();
    library::enqueue(&c, "feed", &feed.0).unwrap();
    intake::run_one(&mut c, &lease, &f).await.unwrap();
    assert_eq!(f.calls.get(), 2);
    assert_eq!(
        c.query_row("SELECT count(*) FROM feed_captures", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert!(library::metadata(&c, &id).unwrap().archived_at.is_some());
}
#[test]
fn imports_keep_previews_and_state_but_no_staged_or_permanent_body() {
    let mut c = db();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("in.json");
    std::fs::write(&path,serde_json::to_vec(&json!({"format":"reading-list-v1","articles":[{"url":"https://example.org/a","title":"Title","html":"<p>Entire source phrase only first few sentences retained.</p>","status":"saved","read":true,"archived":true}],"feeds":[]})).unwrap()).unwrap();
    let run = imports::stage(&mut c, &path).unwrap();
    let staged: String = c
        .query_row("SELECT item FROM import_rows", [], |r| r.get(0))
        .unwrap();
    let staged: serde_json::Value = serde_json::from_str(&staged).unwrap();
    assert_eq!(staged["html"], "");
    imports::start(&mut c, &run, false).unwrap();
    while imports::run_batch(&mut c).unwrap() {}
    let q = Query::for_scope(Scope::Archive);
    let a = library::articles(&c, &q, 0).unwrap().remove(0);
    assert!(a.excerpt.contains("Entire source phrase"));
    assert_eq!(a.word_count, 8);
    assert!(a.read_at.is_some() && a.archived_at.is_some());
    assert!(library::body(&c, &a.id).unwrap().html.is_empty());
    let zip = directory.path().join("out.zip");
    exports::export(&c, &zip).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
    assert!(archive.file_names().all(|n| !n.starts_with("articles/")));
    let manifest: serde_json::Value =
        serde_json::from_reader(archive.by_name("reading-list.json").unwrap()).unwrap();
    assert_eq!(
        manifest["retention"]["policy"],
        "metadata-and-viewer-pdfs-v1"
    );
    assert_eq!(
        manifest["articles"][0]["bodyAvailability"],
        "temporary-cache-not-exported"
    );
}
#[test]
fn metadata_search_never_depends_on_body_presence_and_archive_preserves_preview() {
    let mut c = db();
    let (id, _) = library::save(
        &mut c,
        "https://example.org/a",
        ArrivalKind::Web,
        "save",
        "Saved",
        Some("Title"),
        None,
    )
    .unwrap();
    c.execute(
        "UPDATE articles SET excerpt='Preview needle',word_count=400,status='saved' WHERE id=?1",
        params![id.0],
    )
    .unwrap();
    let mut q = Query::for_scope(Scope::Inbox);
    q.search = "needle".into();
    assert_eq!(library::matching_count(&c, &q).unwrap(), 1);
    q.search = "transient".into();
    assert_eq!(library::matching_count(&c, &q).unwrap(), 0);
    library::update(&c, &id, None, Some(true)).unwrap();
    let body = library::body(&c, &id).unwrap();
    assert!(body.html.is_empty());
    assert_eq!(body.article.excerpt, "Preview needle");
    assert_eq!(body.article.word_count, 400);
}

#[test]
fn archive_fences_racing_cache_publication_and_preserves_metadata() {
    use macos_read_later::storage::{CachedArticle, Storage};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite3");
    let mut c = db_compat::open(&path).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    let (id, _) = library::save(
        &mut c,
        "https://example.org/a",
        ArrivalKind::Web,
        "save",
        "Saved",
        Some("Title"),
        None,
    )
    .unwrap();
    let storage = Storage::for_database(&c).unwrap();
    let epoch = storage.cache_generation(&id.0).unwrap();
    let copy = CachedArticle {
        html: "<p>Disposable current copy.</p>".into(),
        media: vec![],
        status: CopyStatus::Saved,
        error: None,
        word_count: 3,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    };
    assert!(
        storage
            .cache_put(&id.0, epoch, false, copy.clone())
            .unwrap()
    );
    library::update(&c, &id, None, Some(true)).unwrap();
    assert!(storage.cache_get(&id.0).unwrap().is_none());
    assert!(!storage.cache_put(&id.0, epoch, false, copy).unwrap());
    assert_eq!(library::metadata(&c, &id).unwrap().title, "Title");
    assert!(library::metadata(&c, &id).unwrap().read_at.is_some());
}

#[test]
fn import_start_and_removal_serialize_with_intake_before_reading_quota() {
    use rusqlite::{functions::FunctionFlags, types::ValueRef};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    for removal in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite3");
        let mut c = db_compat::open(&path).unwrap();
        db_compat::initialize_lean(&c).unwrap();
        let upload = directory.path().join("import.csv");
        std::fs::write(&upload, "url,title\nhttps://example.org/import,Imported\n").unwrap();
        let run = imports::stage(&mut c, &upload).unwrap();

        // A competing intake connection tries to commit exactly while the
        // import reads its reservation estimate. With a deferred transaction
        // it succeeds, making the following import UPDATE/DELETE fail with
        // SQLITE_BUSY_SNAPSHOT. An immediate transaction already owns the
        // writer, so the competing write defers instead. No timing sleeps or
        // changes to the production reservation code are needed.
        let intake = db_compat::open(&path).unwrap();
        intake.busy_timeout(std::time::Duration::ZERO).unwrap();
        let attempted = Arc::new(AtomicUsize::new(0));
        let observed = attempted.clone();
        c.create_scalar_function(
            "length",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            move |context| {
                if observed.load(Ordering::SeqCst) == 0 {
                    let outcome = intake.execute(
                        "INSERT INTO worker_health(id,heartbeat_at) VALUES(1,'2030-01-01') ON CONFLICT(id) DO UPDATE SET heartbeat_at=excluded.heartbeat_at",
                        [],
                    );
                    let result = match outcome {
                        Err(rusqlite::Error::SqliteFailure(code, _))
                            if code.code == rusqlite::ErrorCode::DatabaseBusy => 1,
                        Ok(_) => 2,
                        Err(_) => 3,
                    };
                    observed.store(result,Ordering::SeqCst);
                }
                Ok(match context.get_raw(0) {
                    ValueRef::Null => None,
                    ValueRef::Blob(bytes) => Some(bytes.len() as i64),
                    ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).chars().count() as i64),
                    ValueRef::Integer(number) => Some(number.to_string().len() as i64),
                    ValueRef::Real(number) => Some(number.to_string().len() as i64),
                })
            },
        ).unwrap();

        if removal {
            imports::remove(&c, &run).unwrap();
            assert!(imports::list(&c).unwrap().is_empty());
        } else {
            imports::start(&mut c, &run, false).unwrap();
            assert_eq!(imports::list(&c).unwrap()[0].state, "queued");
        }
        assert_eq!(
            attempted.load(Ordering::SeqCst),
            1,
            "Intake must defer until the import mutation commits"
        );
        // The writer is released after the lifecycle operation; later intake
        // still works, rather than being left behind a persistent lock.
        let intake = db_compat::open(&path).unwrap();
        intake.busy_timeout(std::time::Duration::ZERO).unwrap();
        intake.execute("INSERT INTO worker_health(id,heartbeat_at) VALUES(1,'2031-01-01') ON CONFLICT(id) DO UPDATE SET heartbeat_at=excluded.heartbeat_at",[]).unwrap();
    }
}

#[test]
fn lean_exports_include_owned_draft_media_and_compact_viewer_provenance() {
    use sha2::{Digest, Sha256};
    let c = db();
    macos_read_later::magazine::initialize(&c).unwrap();
    c.execute(
        "INSERT INTO magazines(id,user_id,title) VALUES('issue',?1,'Issue')",
        [OWNER],
    )
    .unwrap();
    let bytes = b"exact owned production image bytes";
    let digest = format!("{:x}", Sha256::digest(bytes));
    c.execute("INSERT INTO native_magazine_media(magazine_id,source,body,sha256) VALUES('issue','https://example.org/image.png',?1,?2)", params![bytes.as_slice(),digest]).unwrap();
    c.execute("INSERT INTO native_magazine_acquisitions(magazine_id,article_id,source_sha256,retrieved_at,html_sha256) VALUES('issue','article',?1,'2026-10-14T00:00:00Z',?1)", [&digest]).unwrap();
    c.execute(
        "INSERT INTO native_viewer_provenance(magazine_id,provenance) VALUES('issue',?1)",
        [json!({"originalHash":"retained"}).to_string()],
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned.zip");
    exports::export(&c, &path).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let acquisitions: serde_json::Value = serde_json::from_reader(
        archive
            .by_name("tables/native_magazine_acquisitions.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(acquisitions[0]["source_sha256"], digest);
    let provenance: serde_json::Value = serde_json::from_reader(
        archive
            .by_name("tables/native_viewer_provenance.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(provenance[0]["provenance"]["originalHash"], "retained");
    let mut exported = Vec::new();
    std::io::Read::read_to_end(
        &mut archive
            .by_name("binary/native_magazine_media/0-body")
            .unwrap(),
        &mut exported,
    )
    .unwrap();
    assert_eq!(exported, bytes);
}

struct MetadataFetcher {
    calls: Cell<u32>,
}
#[async_trait::async_trait(?Send)]
impl Fetcher for MetadataFetcher {
    async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
        assert!(!image, "Metadata intake must not fetch images");
        self.calls.set(self.calls.get() + 1);
        let prose = "Complete article paragraph words. ".repeat(100);
        Ok(network::Resource {url:url.parse().unwrap(),body:format!("<html><head><title>Fetched title</title></head><body><article><h1>Fetched title</h1><p>{prose}</p><img src='https://example.org/photo.png'><p>Exact final ending.</p></article></body></html>").into_bytes(),content_type:"text/html".into()})
    }
}
#[tokio::test(flavor = "current_thread")]
async fn manual_metadata_fetch_discards_publisher_bytes_and_skips_media() {
    let mut c = db();
    let (id, _) = library::save(
        &mut c,
        "https://example.org/a",
        ArrivalKind::Web,
        "save",
        "Saved",
        None,
        None,
    )
    .unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let fetcher = MetadataFetcher {
        calls: Cell::new(0),
    };
    intake::run_one(&mut c, &lease, &fetcher).await.unwrap();
    assert_eq!(fetcher.calls.get(), 1);
    let body = library::body(&c, &id).unwrap();
    assert_eq!(body.article.title, "Fetched title");
    assert!(body.article.excerpt.contains("Complete article"));
    assert!(body.article.word_count > 300);
    assert!(body.html.is_empty() && body.media.is_empty());
    assert_eq!(
        c.query_row("SELECT count(*) FROM native_article_captures", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
}
