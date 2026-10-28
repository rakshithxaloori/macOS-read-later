use anyhow::Result;
use macos_read_later::{
    db_compat,
    domain::{ArrivalKind, OWNER},
    intake::{self, Fetcher, JobKind},
    library, network,
};
use rusqlite::{Connection, params};
use std::{cell::Cell, collections::HashMap, io::Cursor, sync::Arc, time::Duration};

fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    db_compat::register(&c).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    db_compat::initialize(&c).unwrap();
    intake::initialize(&c).unwrap();
    c
}
fn xml(urls: &[&str]) -> String {
    format!("<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Publisher</title><link>https://example.org</link><description>Feed</description>{}</channel></rss>",urls.iter().map(|u|format!("<item><title>Original headline</title><link>{u}</link><pubDate>Wed, 07 Oct 2026 09:00:00 GMT</pubDate><description><![CDATA[<p>Retained feed excerpt.</p>]]></description></item>")).collect::<String>())
}
struct Fake {
    resources: HashMap<String, network::Resource>,
    calls: Cell<usize>,
}
impl Fake {
    fn none() -> Self {
        Self {
            resources: HashMap::new(),
            calls: Cell::new(0),
        }
    }
    fn page(html: &str) -> Self {
        Self {
            resources: HashMap::from([(
                "https://example.org/a".into(),
                network::Resource {
                    url: "https://example.org/a".parse().unwrap(),
                    body: html.as_bytes().to_vec(),
                    content_type: "text/html".into(),
                },
            )]),
            calls: Cell::new(0),
        }
    }
}
#[async_trait::async_trait(?Send)]
impl Fetcher for Fake {
    async fn fetch(&self, url: &str, _: bool) -> Result<network::Resource> {
        self.calls.set(self.calls.get() + 1);
        self.resources
            .get(url)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Synthetic unavailable resource"))
    }
}
fn prose() -> String {
    let paragraph = (0..85).map(|i| format!("word{i} ")).collect::<String>();
    format!(
        "<html><head><title>Complete publisher article</title></head><body><article><h1>Complete publisher article</h1><p>{paragraph}Opening sentence.</p><p>{paragraph}Middle sentence.</p><p>{paragraph}Final unique sentinel ending.</p></article></body></html>"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn durable_feed_replay_smaller_windows_pause_and_tombstones() {
    let mut c = db();
    let feed = library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    assert_eq!(job.kind, JobKind::Feed);
    let capture = intake::persist_capture(
        &mut c,
        &lease,
        &job,
        &xml(&["https://example.org/a", "https://example.org/b"]),
    )
    .unwrap();
    // Crash before parsing: interrupted work is retried from the retained XML.
    intake::release_worker(&mut c, &lease).unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    c.execute("UPDATE feeds SET active=0 WHERE id=?1", [&feed.0])
        .unwrap();
    let fetcher = Fake::none();
    assert!(intake::run_one(&mut c, &lease, &fetcher).await.unwrap());
    assert_eq!(fetcher.calls.get(), 0);
    assert_eq!(
        c.query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert!(
        c.query_row(
            "SELECT processed_at IS NOT NULL FROM feed_captures WHERE id=?1",
            [&capture],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    let a = macos_read_later::domain::ArticleId(
        c.query_row(
            "SELECT id FROM articles WHERE url='https://example.org/a'",
            [],
            |r| r.get(0),
        )
        .unwrap(),
    );
    let b = macos_read_later::domain::ArticleId(
        c.query_row(
            "SELECT id FROM articles WHERE url='https://example.org/b'",
            [],
            |r| r.get(0),
        )
        .unwrap(),
    );
    library::update(&c, &a, None, Some(true)).unwrap();
    library::delete(&mut c, std::slice::from_ref(&b)).unwrap();
    c.execute("DELETE FROM jobs WHERE kind='article'", [])
        .unwrap();
    library::enqueue(&c, "feed", &feed.0).unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    let duplicate = intake::persist_capture(
        &mut c,
        &lease,
        &job,
        &xml(&["https://example.org/a", "https://example.org/b"]),
    )
    .unwrap();
    assert_eq!(duplicate, capture);
    intake::replay_capture(&mut c, &lease, &job, &duplicate).unwrap();
    intake::finish_job(&mut c, &lease, &job, Ok(None)).unwrap();
    assert!(library::body(&c, &a).unwrap().article.archived_at.is_some());
    assert!(library::body(&c, &b).is_err());
    c.execute("DELETE FROM jobs WHERE kind='article'", [])
        .unwrap();
    library::enqueue(&c, "feed", &feed.0).unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    let smaller =
        intake::persist_capture(&mut c, &lease, &job, &xml(&["https://example.org/a"])).unwrap();
    intake::replay_capture(&mut c, &lease, &job, &smaller).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    intake::finish_job(&mut c, &lease, &job, Ok(None)).unwrap();
    library::update_feed(&mut c, &feed, macos_read_later::domain::FeedAction::Remove).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
}
#[test]
fn lease_expiry_cannot_evict_live_process_and_old_tokens_cannot_publish() {
    let mut c = db();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    c.execute(
        "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'",
        [],
    )
    .unwrap();
    assert!(intake::claim_worker(&mut c).unwrap().is_none());
    assert!(intake::schedule_feeds(&mut c, &lease).is_err());
    intake::renew_worker(&mut c, &lease).unwrap();
    assert!(intake::schedule_feeds(&mut c, &lease).is_ok());
    c.execute("UPDATE worker_leader SET process_id=2147483647", [])
        .unwrap();
    let current = intake::claim_worker(&mut c).unwrap().unwrap();
    assert_ne!(lease.token, current.token);
    assert!(intake::schedule_feeds(&mut c, &lease).is_err());
    assert!(intake::renew_worker(&mut c, &lease).is_err());
    assert!(intake::renew_worker(&mut c, &current).is_ok());
    let feed = library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let old = intake::claim_job(&mut c, &current).unwrap().unwrap();
    c.execute("UPDATE jobs SET lease_until='2000-01-01T00:00:00.000Z'", [])
        .unwrap();
    let new = intake::claim_job(&mut c, &current).unwrap().unwrap();
    assert_ne!(old.token, new.token);
    assert!(
        intake::persist_capture(&mut c, &current, &old, &xml(&["https://example.org/a"])).is_err()
    );
    assert!(intake::finish_job(&mut c, &current, &old, Ok(None)).is_err());
    assert!(
        intake::persist_capture(&mut c, &current, &new, &xml(&["https://example.org/a"])).is_ok()
    );
    assert_eq!(new.target_id, feed.0);
}
#[test]
fn renewal_cannot_revive_released_replaced_or_disabled_owners() {
    let mut c = db();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let wrong_pid = intake::WorkerLease {
        token: lease.token.clone(),
        process_id: lease.process_id.saturating_add(1),
    };
    assert!(intake::renew_worker(&mut c, &wrong_pid).is_err());
    c.execute("UPDATE users SET disabled_at=now() WHERE id=?1", [OWNER])
        .unwrap();
    assert!(intake::renew_worker(&mut c, &lease).is_err());
    c.execute("UPDATE users SET disabled_at=NULL WHERE id=?1", [OWNER])
        .unwrap();
    intake::release_worker(&mut c, &lease).unwrap();
    assert!(intake::renew_worker(&mut c, &lease).is_err());
    let replacement = intake::claim_worker(&mut c).unwrap().unwrap();
    assert!(intake::renew_worker(&mut c, &lease).is_err());
    assert!(intake::renew_worker(&mut c, &replacement).is_ok());
}

#[test]
fn ordinary_heartbeat_preserves_the_current_running_job_and_attempt() {
    let mut c = db();
    library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    intake::renew_worker(&mut c, &lease).unwrap();
    let (state, token, attempts): (String, String, u32) = c
        .query_row(
            "SELECT state,lease_token,attempts FROM jobs WHERE id=?1",
            [&job.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (state.as_str(), token.as_str(), attempts),
        ("running", job.token.as_str(), job.attempts)
    );
    let finished: Option<String> = c
        .query_row(
            "SELECT finished_at FROM native_intake_attempts WHERE token=?1",
            [&job.token],
            |r| r.get(0),
        )
        .unwrap();
    assert!(finished.is_none());
    intake::finish_job(&mut c, &lease, &job, Ok(None)).unwrap();
}

#[test]
fn expired_owner_shutdown_requeues_its_jobs_without_touching_replacements() {
    let mut c = db();
    library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    c.execute(
        "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'",
        [],
    )
    .unwrap();
    intake::release_worker(&mut c, &lease).unwrap();
    let state: String = c
        .query_row("SELECT state FROM jobs WHERE id=?1", [&job.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(state, "queued");
    let replacement = intake::claim_worker(&mut c).unwrap().unwrap();
    let current = intake::claim_job(&mut c, &replacement).unwrap().unwrap();
    intake::release_worker(&mut c, &lease).unwrap();
    assert!(intake::renew_worker(&mut c, &replacement).is_ok());
    let token: String = c
        .query_row(
            "SELECT lease_token FROM jobs WHERE id=?1",
            [&current.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(token, current.token);
}
#[tokio::test(flavor = "current_thread")]
async fn retries_stop_after_five_and_diagnostics_do_not_damage_retained_copy() {
    let mut c = db();
    let (article, _) = library::save(
        &mut c,
        "https://example.org/a",
        ArrivalKind::Web,
        "manual",
        "Saved",
        None,
        None,
    )
    .unwrap();
    let retained = "<p>Useful existing copy with complete text.</p>";
    c.execute("UPDATE articles SET html=?2,status='saved',word_count=7,read_at=now(),archived_at=now() WHERE id=?1",params![article.0,retained]).unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let fetcher = Fake::none();
    for n in 1..=5 {
        assert!(intake::run_one(&mut c, &lease, &fetcher).await.unwrap());
        assert_eq!(
            c.query_row("SELECT attempts FROM jobs", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            n
        );
        c.execute("UPDATE jobs SET run_at=now()", []).unwrap();
    }
    assert!(!intake::run_one(&mut c, &lease, &fetcher).await.unwrap());
    let state: String = c
        .query_row("SELECT state FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "failed");
    let body = library::body(&c, &article).unwrap();
    assert_eq!(body.html, retained);
    assert!(body.article.read_at.is_some() && body.article.archived_at.is_some());
    assert_eq!(
        body.article.status,
        macos_read_later::domain::CopyStatus::Saved
    );
    assert!(body.article.error.is_none());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM native_intake_attempts WHERE finished_at IS NOT NULL",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        5
    );
}
#[tokio::test(flavor = "current_thread")]
async fn complete_extraction_preserves_source_hash_and_caches_only_valid_raster() {
    let mut c = db();
    let (article, _) = library::save(
        &mut c,
        "https://example.org/a",
        ArrivalKind::Web,
        "manual",
        "Saved",
        None,
        None,
    )
    .unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let source=prose().replace("</article>","<figure><img src='https://example.org/valid.png' alt='Source illustration'><figcaption>Exact caption</figcaption></figure><figure><img src='https://example.org/corrupt.png' alt='Corrupt source'></figure></article>");
    let mut fetcher = Fake::page(&source);
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    for (url, body) in [
        ("https://example.org/valid.png", png.into_inner()),
        (
            "https://example.org/corrupt.png",
            b"<svg>executable unsupported image</svg>".to_vec(),
        ),
    ] {
        fetcher.resources.insert(
            url.into(),
            network::Resource {
                url: url.parse().unwrap(),
                body,
                content_type: "image/png".into(),
            },
        );
    }
    assert!(intake::run_one(&mut c, &lease, &fetcher).await.unwrap());
    let body = library::body(&c, &article).unwrap();
    assert!(body.html.contains("/api/media/"));
    assert!(!body.html.contains("<img src=\"https:"));
    assert!(body.html.contains("Final unique sentinel ending."));
    assert!(
        body.html
            .contains("[Image unavailable: Corrupt source. View original]")
    );
    assert_eq!(body.media.len(), 1);
    let captured: Vec<u8> = c
        .query_row("SELECT body FROM native_article_captures", [], |r| r.get(0))
        .unwrap();
    assert_eq!(captured, source.as_bytes());
    let hash: String = c
        .query_row("SELECT sha256 FROM native_article_captures", [], |r| {
            r.get(0)
        })
        .unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(hash, format!("{:x}", Sha256::digest(source.as_bytes())));
}
#[tokio::test(flavor = "current_thread")]
async fn shorter_complete_capture_verifies_retained_copy_without_replacing_extra_prose() {
    let mut c = db();
    let (article, _) = library::save(
        &mut c,
        "https://example.org/a",
        ArrivalKind::Web,
        "save",
        "Saved",
        None,
        None,
    )
    .unwrap();
    let source = prose();
    let incoming = macos_read_later::extraction::extract(&source, "https://example.org/a").unwrap();
    assert_eq!(incoming.status, macos_read_later::domain::CopyStatus::Saved);
    let retained = format!(
        "<h2>Extra retained heading</h2>{}<p>Extra final retained paragraph.</p>",
        incoming.html
    );
    c.execute("UPDATE articles SET html=?2,status='partial',word_count=?3,error='Feed copy not verified' WHERE id=?1",params![article.0,retained,macos_read_later::extraction::word_count(&retained)]).unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    assert!(
        intake::run_one(&mut c, &lease, &Fake::page(&source))
            .await
            .unwrap()
    );
    let body = library::body(&c, &article).unwrap();
    assert_eq!(body.html, retained);
    assert_eq!(
        body.article.status,
        macos_read_later::domain::CopyStatus::Saved
    );
    assert!(body.article.error.is_none());
    let diagnostic: Option<String> = c
        .query_row("SELECT error FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert!(diagnostic.is_none());
}
struct DisplaceDuringFetch {
    path: std::path::PathBuf,
    source: String,
    mutation: &'static str,
}
struct SuspendDuringFetch {
    path: std::path::PathBuf,
    expire_process: bool,
    expire_job: bool,
    timeout: bool,
    heartbeat_first: Option<intake::WorkerLease>,
}
#[async_trait::async_trait(?Send)]
impl Fetcher for SuspendDuringFetch {
    async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
        anyhow::ensure!(!image, "No image requested by this fixture");
        let mut c = db_compat::open(&self.path)?;
        if self.expire_process {
            c.execute(
                "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'",
                [],
            )?;
        }
        if self.expire_job {
            c.execute(
                "UPDATE jobs SET lease_until='2000-01-01T00:00:00.000Z' WHERE state='running'",
                [],
            )?;
        }
        if let Some(lease) = &self.heartbeat_first {
            intake::renew_worker(&mut c, lease)?;
        }
        anyhow::ensure!(!self.timeout, "Synthetic request timed out during sleep");
        Ok(network::Resource {
            url: url.parse()?,
            body: prose().into_bytes(),
            content_type: "text/html".into(),
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn suspended_fetch_recovers_before_heartbeat_without_spending_failure_budget() {
    use std::os::unix::fs::PermissionsExt;
    for (expire_process, expire_job, timeout, heartbeat_first) in [
        (true, false, false, false),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, true, true),
        (true, true, false, true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("library.sqlite3");
        let mut c = db_compat::open(&path).unwrap();
        db_compat::initialize(&c).unwrap();
        intake::initialize(&c).unwrap();
        let (article, _) = library::save(
            &mut c,
            "https://example.org/a",
            ArrivalKind::Web,
            "save",
            "Saved",
            None,
            None,
        )
        .unwrap();
        c.execute("UPDATE jobs SET attempts=4", []).unwrap();
        let lease = intake::claim_worker(&mut c).unwrap().unwrap();
        assert!(
            intake::run_one(
                &mut c,
                &lease,
                &SuspendDuringFetch {
                    path,
                    expire_process,
                    expire_job,
                    timeout,
                    heartbeat_first: heartbeat_first.then(|| lease.clone()),
                }
            )
            .await
            .unwrap()
        );
        // The resumed request reached the publishing boundary before a heartbeat.
        // Its expired ownership must not publish or turn attempt five into failure.
        assert!(library::body(&c, &article).unwrap().html.is_empty());
        assert_eq!(
            c.query_row("SELECT count(*) FROM native_article_captures", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
        let (state, attempts): (String, u32) = c
            .query_row("SELECT state,attempts FROM jobs", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((state.as_str(), attempts), ("queued", 4));
        let outcome: String = c
            .query_row("SELECT outcome FROM native_intake_attempts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(outcome, "interrupted");
        intake::schedule_feeds(&mut c, &lease).unwrap();
        assert!(
            intake::run_one(&mut c, &lease, &Fake::page(&prose()))
                .await
                .unwrap()
        );
        assert!(!library::body(&c, &article).unwrap().html.is_empty());
        let state: String = c
            .query_row("SELECT state FROM jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "done");
    }
}
#[async_trait::async_trait(?Send)]
impl Fetcher for DisplaceDuringFetch {
    async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
        anyhow::ensure!(!image, "No image requested by this fixture");
        let c = db_compat::open(&self.path)?;
        c.execute_batch(self.mutation)?;
        Ok(network::Resource {
            url: url.parse()?,
            body: self.source.as_bytes().to_vec(),
            content_type: "text/html".into(),
        })
    }
}
#[tokio::test(flavor = "current_thread")]
async fn stale_completion_during_network_cannot_publish_article_or_original_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("library.sqlite3");
    let mut c = db_compat::open(&path).unwrap();
    db_compat::initialize(&c).unwrap();
    intake::initialize(&c).unwrap();
    let (article, _) = library::save(
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
    let fetcher = DisplaceDuringFetch {
        path,
        source: prose(),
        mutation: "UPDATE jobs SET lease_token='replacement-token',lease_until=date_add(now(),2,'minutes') WHERE state='running'",
    };
    assert!(intake::run_one(&mut c, &lease, &fetcher).await.unwrap());
    assert!(library::body(&c, &article).unwrap().html.is_empty());
    assert_eq!(
        c.query_row("SELECT count(*) FROM native_article_captures", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    let token: String = c
        .query_row("SELECT lease_token FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(token, "replacement-token");
}
#[tokio::test(flavor = "current_thread")]
async fn resumed_result_cannot_reclaim_replaced_worker_or_job() {
    use std::os::unix::fs::PermissionsExt;
    for (mutation, worker_replaced) in [
        ("UPDATE worker_leader SET token='replacement-worker'", true),
        (
            "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'; UPDATE jobs SET lease_token='replacement-token',lease_until=date_add(now(),2,'minutes') WHERE state='running'",
            false,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("library.sqlite3");
        let mut c = db_compat::open(&path).unwrap();
        db_compat::initialize(&c).unwrap();
        intake::initialize(&c).unwrap();
        let (article, _) = library::save(
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
        let result = intake::run_one(
            &mut c,
            &lease,
            &DisplaceDuringFetch {
                path,
                source: prose(),
                mutation,
            },
        )
        .await;
        assert_eq!(result.is_err(), worker_replaced);
        assert!(library::body(&c, &article).unwrap().html.is_empty());
        assert_eq!(
            c.query_row("SELECT count(*) FROM native_article_captures", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
        let (state, token): (String, String) = c
            .query_row("SELECT state,lease_token FROM jobs", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(state, "running");
        if worker_replaced {
            assert_ne!(token, "replacement-token");
            let worker_token: String = c
                .query_row("SELECT token FROM worker_leader", [], |r| r.get(0))
                .unwrap();
            assert_eq!(worker_token, "replacement-worker");
        } else {
            assert_eq!(token, "replacement-token");
            intake::renew_worker(&mut c, &lease).unwrap();
        }
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM native_intake_attempts WHERE finished_at IS NOT NULL",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
            0
        );
    }
}
#[test]
fn image_repair_requires_unique_exact_markers() {
    let id = uuid::Uuid::new_v4();
    let incoming = format!("<a href='https://example.org/photo'><img src='/api/media/{id}'></a>");
    assert!(
        intake::merge_reading_images(
            "<p>Richer prose</p><a href='https://example.org/photo'></a>",
            &incoming,
            "https://example.org/a"
        )
        .unwrap()
        .contains("/api/media/")
    );
    assert!(
        intake::merge_reading_images(
            "<a href='https://example.org/photo'>Source prose</a>",
            &incoming,
            "https://example.org/a"
        )
        .is_none()
    );
    assert!(
        intake::merge_reading_images(
            "<a href='https://example.org/photo'></a><a href='https://example.org/photo'></a>",
            &incoming,
            "https://example.org/a"
        )
        .is_none()
    );
    let figure = format!(
        "<figure><img src='/api/media/{id}'><figcaption>Unique exact caption</figcaption></figure>"
    );
    assert!(intake::merge_reading_images("<figure><a href='https://example.org/a'>[Image unavailable. View original]</a><figcaption>Unique exact caption</figcaption></figure>",&figure,"https://example.org/a").unwrap().contains("/api/media/"));
}
#[test]
fn entities_rejected_capture_retained_and_scheduler_does_not_loop_failed_replay() {
    let mut c = db();
    let feed = library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let lease = intake::claim_worker(&mut c).unwrap().unwrap();
    let job = intake::claim_job(&mut c, &lease).unwrap().unwrap();
    let source = "<!DOCTYPE rss [<!ENTITY x SYSTEM 'file:///private/secret'>]><rss/>";
    let id = intake::persist_capture(&mut c, &lease, &job, source).unwrap();
    assert!(intake::replay_capture(&mut c, &lease, &job, &id).is_err());
    intake::finish_job(&mut c, &lease, &job, Err(anyhow::anyhow!("Invalid feed"))).unwrap();
    c.execute("UPDATE jobs SET state='failed',attempts=5", [])
        .unwrap();
    c.execute("UPDATE feeds SET active=0 WHERE id=?1", [feed.0])
        .unwrap();
    intake::schedule_feeds(&mut c, &lease).unwrap();
    assert!(
        !c.query_row("SELECT state='queued' FROM jobs", [], |r| r
            .get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        c.query_row("SELECT xml FROM feed_captures", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        source
    );
}
#[test]
fn actor_starts_disabled_and_quits_without_polling_live_feeds() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("library.sqlite3");
    let mut c = db_compat::open(&path).unwrap();
    db_compat::initialize(&c).unwrap();
    library::add_feed(&mut c, "https://example.org/feed").unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let handle = intake::spawn(
        intake::Config {
            library_path: path,
            private_directory: directory.path().to_owned(),
        },
        tx,
        Arc::new(|| {}),
    )
    .unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    handle.shutdown();
    assert_eq!(
        c.query_row("SELECT count(*) FROM feed_captures", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM worker_leader", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(OWNER, "00000000-0000-4000-8000-000000000001");
}
#[test]
fn actor_file_lock_prevents_competing_owner_and_shutdown_releases_it() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("library.sqlite3");
    let c = db_compat::open(&path).unwrap();
    db_compat::initialize(&c).unwrap();
    let config = || intake::Config {
        library_path: path.clone(),
        private_directory: directory.path().to_owned(),
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let first = intake::spawn(config(), tx, Arc::new(|| {})).unwrap();
    first.command(intake::Command::Enable(true)).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        intake::Event::State {
            enabled: true,
            error: None
        }
    ));
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let second = intake::spawn(config(), tx, Arc::new(|| {})).unwrap();
    second.command(intake::Command::Enable(true)).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        intake::Event::State {
            enabled: false,
            error: Some(_)
        }
    ));
    first.shutdown();
    second.command(intake::Command::Enable(true)).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        intake::Event::State {
            enabled: true,
            error: None
        }
    ));
    second.shutdown();
    assert_eq!(
        c.query_row("SELECT count(*) FROM worker_leader", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn fresh_compact_root_creates_private_intake_directory_and_starts_paused() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite3");
    let mut c = db_compat::open(&path).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    let feed = library::add_feed(&mut c, "https://example.org/paused-feed").unwrap();
    library::update_feed(&mut c, &feed, macos_read_later::domain::FeedAction::Pause).unwrap();
    // A paused subscription needs no initial queued fetch. No network work is
    // permitted in this startup regression.
    c.execute("DELETE FROM jobs", []).unwrap();
    let private_directory = directory.path().join("intake");
    assert!(!private_directory.exists());
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let handle = intake::spawn(
        intake::Config {
            library_path: path,
            private_directory: private_directory.clone(),
        },
        tx,
        Arc::new(|| {}),
    )
    .unwrap();
    handle.command(intake::Command::Enable(true)).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        intake::Event::State {
            enabled: true,
            error: None
        }
    ));
    handle.shutdown();
    let metadata = std::fs::symlink_metadata(&private_directory).unwrap();
    assert!(metadata.file_type().is_dir());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    assert!(private_directory.join("intake.lock").is_file());
    assert_eq!(
        c.query_row("SELECT count(*) FROM feed_captures", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM worker_leader", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn intake_startup_rejects_existing_and_dangling_directory_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("outside");
    std::fs::create_dir(&target).unwrap();
    for (name, destination) in [
        ("existing", target.clone()),
        ("dangling", directory.path().join("missing")),
    ] {
        let private_directory = directory.path().join(name);
        std::os::unix::fs::symlink(&destination, &private_directory).unwrap();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        assert!(
            intake::spawn(
                intake::Config {
                    library_path: directory.path().join("unused.sqlite3"),
                    private_directory
                },
                tx,
                Arc::new(|| {})
            )
            .is_err()
        );
        assert!(!destination.join("intake.lock").exists());
    }
    assert!(!directory.path().join("unused.sqlite3").exists());
}
