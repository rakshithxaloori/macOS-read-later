//! Durable RSS/article intake. Network futures never hold a SQLite transaction.
use crate::{
    db_compat,
    domain::{ArrivalKind, CopyStatus, OWNER},
    extraction, library, network,
};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};
use tokio::sync::mpsc;

pub struct Config {
    pub library_path: PathBuf,
    pub private_directory: PathBuf,
}
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Enable(bool),
    Wake,
    Quit,
}
#[derive(Clone, Debug)]
pub enum Event {
    LibraryChanged,
    State {
        enabled: bool,
        error: Option<String>,
    },
}
pub struct Handle {
    commands: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Handle {
    pub fn command(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .context("Intake command queue is full or closed")
    }
    pub fn shutdown(mut self) {
        let _ = self.commands.blocking_send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[derive(Clone, Debug)]
pub struct WorkerLease {
    pub token: String,
    pub process_id: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobKind {
    Feed,
    Article,
}
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub target_id: String,
    pub token: String,
    pub attempts: u32,
    pub kind: JobKind,
}

pub fn initialize(c: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS native_intake_schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT(now())); CREATE TABLE IF NOT EXISTS native_intake_attempts (job_id TEXT NOT NULL,token TEXT PRIMARY KEY,process_token TEXT NOT NULL,kind TEXT NOT NULL,target_id TEXT NOT NULL,attempt INTEGER NOT NULL,started_at TEXT NOT NULL DEFAULT(now()),finished_at TEXT,outcome TEXT,diagnostic TEXT); CREATE TABLE IF NOT EXISTS native_article_captures (id TEXT PRIMARY KEY,user_id TEXT NOT NULL,article_id TEXT NOT NULL,source_url TEXT NOT NULL,sha256 TEXT NOT NULL,body BLOB NOT NULL CHECK(length(body)<=5242880),received_at TEXT NOT NULL DEFAULT(now()),UNIQUE(user_id,article_id,source_url,sha256),FOREIGN KEY(user_id,article_id) REFERENCES articles(user_id,id) ON DELETE CASCADE); INSERT OR IGNORE INTO native_intake_schema_migrations(version) VALUES(1);")?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS native_feed_state(user_id TEXT NOT NULL,feed_id TEXT NOT NULL,content_hash TEXT NOT NULL DEFAULT(''),etag TEXT,last_modified TEXT,PRIMARY KEY(user_id,feed_id),FOREIGN KEY(user_id,feed_id) REFERENCES feeds(user_id,id) ON DELETE CASCADE)")?;
    tx.commit()?;
    Ok(())
}
fn alive(pid: u32) -> bool {
    // A timer expiry never displaces a living local owner. This is the macOS/POSIX
    // process API, not an external runtime helper.
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    i32::try_from(pid).ok().is_some_and(|pid| unsafe { kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() != Some(3))
}
pub fn claim_worker(c: &mut Connection) -> Result<Option<WorkerLease>> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<u32> = tx
        .query_row("SELECT process_id FROM worker_leader WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()?;
    if existing.is_some_and(alive) {
        return Ok(None);
    }
    let lease = WorkerLease {
        token: uuid::Uuid::new_v4().to_string(),
        process_id: std::process::id(),
    };
    tx.execute("INSERT INTO worker_leader(id,token,process_id,heartbeat_at,lease_until) VALUES(1,?1,?2,now(),date_add(now(),30)) ON CONFLICT(id) DO UPDATE SET token=excluded.token,process_id=excluded.process_id,heartbeat_at=excluded.heartbeat_at,lease_until=excluded.lease_until", params![lease.token,lease.process_id])?;
    tx.commit()?;
    Ok(Some(lease))
}
#[derive(Debug)]
struct WorkerLeaseExpired;
impl std::fmt::Display for WorkerLeaseExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Intake process lost its lease")
    }
}
impl std::error::Error for WorkerLeaseExpired {}

fn owned(c: &Connection, lease: &WorkerLease) -> Result<()> {
    let unexpired = c
        .query_row(
            "SELECT lease_until>now() FROM worker_leader WHERE id=1 AND token=?1 AND process_id=?2",
            params![lease.token, lease.process_id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .context("Intake process lost its lease")?;
    if !unexpired {
        return Err(WorkerLeaseExpired.into());
    }
    library_available(c)
}
fn library_available(c: &Connection) -> Result<()> {
    ensure!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND disabled_at IS NULL)",
            [OWNER],
            |r| r.get::<_, bool>(0)
        )?,
        "The local library is unavailable"
    );
    Ok(())
}
pub fn renew_worker(c: &mut Connection, lease: &WorkerLease) -> Result<()> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // The file lock and exact process token/PID still fence this owner after
    // sleep. Expiration blocks publishing, but must not prevent its heartbeat.
    let expired = tx
        .query_row(
            "SELECT lease_until<=now() FROM worker_leader WHERE id=1 AND token=?1 AND process_id=?2",
            params![lease.token, lease.process_id],
            |r| r.get::<_, bool>(0)
        )
        .optional()?
        .context("Intake process lost its lease")?;
    library_available(&tx)?;
    // A pause interrupts work; it does not spend a publisher-failure attempt.
    // Fence out those requests before renewing so heartbeat-first and
    // response-first wakeups both discard their stale completions safely.
    if expired {
        tx.execute("UPDATE native_intake_attempts SET finished_at=now(),outcome='interrupted' WHERE process_token=?1 AND finished_at IS NULL AND EXISTS(SELECT 1 FROM jobs j WHERE j.id=native_intake_attempts.job_id AND j.state='running' AND j.lease_token=native_intake_attempts.token)", [&lease.token])?;
        tx.execute("UPDATE jobs SET state='queued',attempts=MAX(attempts-1,0),lease_until=NULL,lease_token=NULL,run_at=now() WHERE state='running' AND EXISTS(SELECT 1 FROM native_intake_attempts a WHERE a.job_id=jobs.id AND a.token=jobs.lease_token AND a.process_token=?1)", [&lease.token])?;
    }
    ensure!(tx.execute("UPDATE worker_leader SET heartbeat_at=now(),lease_until=date_add(now(),30) WHERE id=1 AND token=?1 AND process_id=?2",params![lease.token,lease.process_id])?==1,"Intake process lost its lease");
    tx.execute("INSERT INTO worker_health(id,heartbeat_at) VALUES(1,now()) ON CONFLICT(id) DO UPDATE SET heartbeat_at=excluded.heartbeat_at",[])?;
    tx.commit()?;
    Ok(())
}
fn resume_expired_worker(
    c: &mut Connection,
    lease: &WorkerLease,
    error: anyhow::Error,
) -> Result<()> {
    if error.is::<WorkerLeaseExpired>() {
        renew_worker(c, lease)
    } else {
        Err(error)
    }
}
pub fn release_worker(c: &mut Connection, lease: &WorkerLease) -> Result<()> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Only jobs actually claimed by this token are recovered on graceful stop.
    let current: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_leader WHERE id=1 AND token=?1 AND process_id=?2)",
        params![lease.token, lease.process_id],
        |r| r.get(0),
    )?;
    if current {
        tx.execute("UPDATE jobs SET state='queued',lease_until=NULL,lease_token=NULL,run_at=now() WHERE state='running' AND EXISTS(SELECT 1 FROM native_intake_attempts a WHERE a.token=jobs.lease_token AND a.process_token=?1)",[&lease.token])?;
        tx.execute("UPDATE native_intake_attempts SET finished_at=now(),outcome='interrupted' WHERE process_token=?1 AND finished_at IS NULL",[&lease.token])?;
    }
    tx.execute(
        "DELETE FROM worker_leader WHERE id=1 AND token=?1 AND process_id=?2",
        params![lease.token, lease.process_id],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn schedule_feeds(c: &mut Connection, lease: &WorkerLease) -> Result<usize> {
    let _reservation = library::reserve_write(c, 524_288)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    owned(&tx, lease)?;
    let ids = tx.prepare("SELECT id FROM feeds WHERE user_id=?1 AND active=1 AND next_poll_at<=now() ORDER BY next_poll_at,id LIMIT 100")?.query_map([OWNER],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for id in &ids {
        library::enqueue(&tx, "feed", id)?;
        tx.execute(
            "UPDATE feeds SET next_poll_at=date_add(now(),15,'minutes') WHERE user_id=?1 AND id=?2",
            params![OWNER, id],
        )?;
    }
    // A capture survives a crash independently of its original queued job. Paused
    // subscriptions still replay captures already received; removed feeds cannot.
    let pending = tx.prepare("SELECT DISTINCT f.id FROM feeds f JOIN feed_captures x ON x.feed_id=f.id AND x.user_id=f.user_id WHERE f.user_id=?1 AND x.processed_at IS NULL AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.user_id=f.user_id AND j.kind='feed' AND j.target_id=f.id AND j.state='failed') LIMIT 100")?.query_map([OWNER],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for id in pending {
        library::enqueue(&tx, "feed", &id)?;
    }
    tx.commit()?;
    Ok(ids.len())
}
pub fn claim_job(c: &mut Connection, lease: &WorkerLease) -> Result<Option<Job>> {
    let _reservation = library::reserve_write(c, 131_072)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    owned(&tx, lease)?;
    let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE user_id=?1 AND state='running' AND lease_until>=now())",[OWNER],|r|r.get(0))?;
    if live {
        return Ok(None);
    }
    let candidate:Option<(String,String,String,u32)>=tx.query_row("SELECT id,target_id,kind,attempts FROM jobs WHERE user_id=?1 AND ((state='queued' AND run_at<=now()) OR (state='running' AND (lease_until IS NULL OR lease_until<now()))) ORDER BY CASE WHEN kind='feed' AND EXISTS(SELECT 1 FROM feed_captures x WHERE x.user_id=jobs.user_id AND x.feed_id=jobs.target_id AND x.processed_at IS NULL) THEN 0 ELSE 1 END,run_at,id LIMIT 1",[OWNER],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((id, target_id, kind, attempts)) = candidate else {
        return Ok(None);
    };
    let token = uuid::Uuid::new_v4().to_string();
    tx.execute("UPDATE native_intake_attempts SET finished_at=now(),outcome='interrupted' WHERE job_id=?1 AND finished_at IS NULL",[&id])?;
    let job = Job {
        id,
        target_id,
        kind: match kind.as_str() {
            "feed" => JobKind::Feed,
            "article" => JobKind::Article,
            _ => bail!("Unsupported intake job"),
        },
        token,
        attempts: attempts.saturating_add(1),
    };
    tx.execute("UPDATE jobs SET state='running',attempts=?2,lease_until=date_add(now(),2,'minutes'),lease_token=?3 WHERE id=?1",params![job.id,job.attempts,job.token])?;
    tx.execute("INSERT INTO native_intake_attempts(job_id,token,process_token,kind,target_id,attempt) VALUES(?1,?2,?3,?4,?5,?6)",params![job.id,job.token,lease.token,kind,job.target_id,job.attempts])?;
    tx.commit()?;
    Ok(Some(job))
}
fn job_owned(c: &Connection, lease: &WorkerLease, job: &Job) -> Result<()> {
    owned(c, lease)?;
    ensure!(c.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE user_id=?1 AND id=?2 AND state='running' AND lease_token=?3 AND lease_until>now())",params![OWNER,job.id,job.token],|r|r.get::<_,bool>(0))?,"Intake job lost its lease");
    Ok(())
}
pub fn finish_job(
    c: &mut Connection,
    lease: &WorkerLease,
    job: &Job,
    result: Result<Option<String>>,
) -> Result<()> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    job_owned(&tx, lease, job)?;
    let (state, error) = match result {
        Ok(diagnostic) => ("done", diagnostic),
        Err(e) => (
            if job.attempts >= 5 {
                "failed"
            } else {
                "queued"
            },
            Some(e.to_string().chars().take(500).collect()),
        ),
    };
    let delay = (30u32.saturating_mul(2u32.saturating_pow(job.attempts.min(10)))).min(3600);
    tx.execute("UPDATE jobs SET state=?3,error=?4,run_at=date_add(now(),?5),lease_until=NULL WHERE id=?1 AND lease_token=?2",params![job.id,job.token,state,error,delay])?;
    tx.execute("UPDATE native_intake_attempts SET finished_at=now(),outcome=?2,diagnostic=?3 WHERE token=?1",params![job.token,state,error])?;
    if db_compat::is_lean(&tx)? {
        tx.execute("DELETE FROM native_intake_attempts WHERE finished_at IS NOT NULL AND token NOT IN (SELECT token FROM native_intake_attempts WHERE finished_at IS NOT NULL ORDER BY finished_at DESC,token LIMIT 100)", [])?;
    }
    if state != "done" {
        match job.kind {
            JobKind::Feed => {
                tx.execute(
                    "UPDATE feeds SET error=?3 WHERE user_id=?1 AND id=?2",
                    params![OWNER, job.target_id, error],
                )?;
            }
            JobKind::Article => {
                if db_compat::is_lean(&tx)? {
                    tx.execute("UPDATE articles SET status='failed',error=?3 WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL AND status IN ('pending','failed')",params![OWNER,job.target_id,error])?;
                } else {
                    tx.execute("UPDATE articles SET status='failed',error=?3 WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL AND html=''",params![OWNER,job.target_id,error])?;
                }
            }
        }
    }
    tx.commit()?;
    Ok(())
}

#[async_trait::async_trait(?Send)]
pub trait Fetcher {
    async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource>;
    async fn fetch_feed(
        &self,
        url: &str,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> Result<network::ConditionalResource> {
        Ok(network::ConditionalResource {
            resource: Some(self.fetch(url, false).await?),
            etag: None,
            last_modified: None,
        })
    }
}
pub struct PublicFetcher;
#[async_trait::async_trait(?Send)]
impl Fetcher for PublicFetcher {
    async fn fetch(&self, url: &str, image: bool) -> Result<network::Resource> {
        network::fetch(url, image).await
    }
    async fn fetch_feed(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<network::ConditionalResource> {
        network::fetch_conditional(url, etag, last_modified).await
    }
}

pub fn persist_capture(
    c: &mut Connection,
    lease: &WorkerLease,
    job: &Job,
    xml: &str,
) -> Result<String> {
    ensure!(xml.len() <= network::MAX_RESPONSE, "Feed exceeds 5 MB");
    let _reservation = library::reserve_write(c, xml.len() as u64 * 3 + 262_144)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    job_owned(&tx, lease, job)?;
    let prior:Option<String>=tx.query_row("SELECT id FROM feed_captures WHERE user_id=?1 AND feed_id=?2 AND content_hash=md5(?3) AND xml=?3 LIMIT 1",params![OWNER,job.target_id,xml],|r|r.get(0)).optional()?;
    if prior.is_none() && db_compat::is_lean(&tx)? {
        let pending: i64 = tx.query_row("SELECT coalesce(sum(length(CAST(xml AS BLOB))),0) FROM feed_captures WHERE processed_at IS NULL", [], |r| r.get(0))?;
        ensure!(
            u64::try_from(pending)?.saturating_add(xml.len() as u64) <= 5_242_880,
            "RSS replay buffer is full; process pending arrivals before fetching more feeds"
        );
    }
    ensure!(
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM feeds WHERE user_id=?1 AND id=?2)",
            params![OWNER, job.target_id],
            |r| r.get::<_, bool>(0)
        )?,
        "Feed was unsubscribed"
    );
    let id = if let Some(id) = prior {
        id
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO feed_captures(id,user_id,feed_id,xml) VALUES(?1,?2,?3,?4)",
            params![id, OWNER, job.target_id, xml],
        )?;
        id
    };
    tx.commit()?;
    Ok(id)
}
pub fn replay_capture(c: &mut Connection, lease: &WorkerLease, job: &Job, id: &str) -> Result<()> {
    let (xml,url):(String,String)=c.query_row("SELECT x.xml,f.url FROM feed_captures x JOIN feeds f ON f.id=x.feed_id AND f.user_id=x.user_id WHERE x.user_id=?1 AND x.id=?2 AND x.feed_id=?3 AND length(CAST(x.xml AS BLOB))<=5242880",params![OWNER,id,job.target_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let upper = xml.to_ascii_uppercase();
    ensure!(
        !upper.contains("<!DOCTYPE") && !upper.contains("<!ENTITY"),
        "Feeds containing XML entities are unsupported"
    );
    let parsed = feed_rs::parser::parse(xml.as_bytes()).context("Feed XML could not be parsed")?;
    ensure!(parsed.entries.len() <= 5000, "Feed exceeds 5,000 entries");
    let base = url::Url::parse(&url)?;
    let _reservation = library::reserve_write(c, xml.len() as u64 * 4 + 1_048_576)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    job_owned(&tx, lease, job)?;
    // Removal while parsing prevents publication. Pause preserves received arrivals.
    ensure!(
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM feeds WHERE user_id=?1 AND id=?2)",
            params![OWNER, job.target_id],
            |r| r.get::<_, bool>(0)
        )?,
        "Feed was unsubscribed"
    );
    let lean = db_compat::is_lean(&tx)?;
    let title = parsed
        .title
        .map(|t| t.content.chars().take(500).collect::<String>());
    for entry in parsed.entries {
        let Some(link) = entry
            .links
            .iter()
            .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
        else {
            continue;
        };
        let Ok(url) = base
            .join(&link.href)
            .and_then(|u| url::Url::parse(u.as_str()))
        else {
            continue;
        };
        let Ok(url) = network::normalize_url(url.as_str()) else {
            continue;
        };
        let published = entry
            .published
            .or(entry.updated)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        let item_title = entry
            .title
            .map(|t| t.content.chars().take(1000).collect::<String>());
        let key = format!("{}:{}", job.target_id, url);
        let (article, _) = library::save_in_transaction(
            &tx,
            url.as_str(),
            ArrivalKind::Rss,
            &key,
            title.as_deref().unwrap_or("RSS"),
            item_title.as_deref(),
            published.as_deref(),
        )?;
        let body = entry
            .content
            .and_then(|c| c.body)
            .or_else(|| entry.summary.map(|t| t.content))
            .unwrap_or_default();
        // Feed copies remain partial until publisher extraction verifies them. No
        // remote image is left in saved content, and duplicates preserve read state.
        let html = extraction::clean_html(&body, url.as_str(), false);
        let words = extraction::word_count(&html);
        let excerpt = extraction::text(&html)
            .chars()
            .take(260)
            .collect::<String>();
        if lean {
            let author = entry
                .authors
                .first()
                .map(|a| a.name.chars().take(500).collect::<String>());
            tx.execute("UPDATE articles SET excerpt=CASE WHEN excerpt='' THEN ?3 ELSE excerpt END,author=COALESCE(author,?4),status=CASE WHEN status='pending' AND ?5>0 THEN 'partial' ELSE status END WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL",params![OWNER,article.0,excerpt,author,words])?;
        } else if words > 0 {
            tx.execute("UPDATE articles SET html=?3,word_count=?4,excerpt=?5,status='partial' WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL AND status<>'saved' AND word_count<?4",params![OWNER,article.0,html,words,excerpt])?;
        }
    }
    tx.execute("UPDATE feeds SET title=COALESCE(?3,title),last_polled_at=now(),error=NULL WHERE user_id=?1 AND id=?2",params![OWNER,job.target_id,title])?;
    if lean {
        tx.execute("INSERT INTO native_feed_state(user_id,feed_id,content_hash) VALUES(?1,?2,?3) ON CONFLICT(user_id,feed_id) DO UPDATE SET content_hash=excluded.content_hash",params![OWNER,job.target_id,format!("{:x}",Sha256::digest(xml.as_bytes()))])?;
        tx.execute(
            "DELETE FROM feed_captures WHERE user_id=?1 AND id=?2",
            params![OWNER, id],
        )?;
    } else {
        tx.execute(
            "UPDATE feed_captures SET processed_at=now() WHERE user_id=?1 AND id=?2",
            params![OWNER, id],
        )?;
    }
    tx.commit()?;
    if lean {
        // Reclamation is maintenance after committed arrivals. A busy reader or
        // a filesystem error cannot make successfully processed XML replayable.
        let _ = db_compat::reclaim(c);
    }
    Ok(())
}
struct CachedImage {
    id: String,
    source: String,
    digest: String,
    content_type: String,
    body: Vec<u8>,
}
struct MediaCopy {
    html: String,
    logo: Option<String>,
    missing: usize,
    images: Vec<CachedImage>,
}
/// Fill only exact unique publisher image-link/caption markers. Never infer an
/// image position from surrounding prose or replace an existing cached image.
pub fn merge_reading_images(retained: &str, incoming: &str, page: &str) -> Option<String> {
    let retained_dom = Html::parse_fragment(retained);
    let incoming_dom = Html::parse_fragment(incoming);
    let images = Selector::parse("img").expect("static selector");
    let links = Selector::parse("a[href]").expect("static selector");
    let figures = Selector::parse("figure").expect("static selector");
    let captions = Selector::parse("figcaption").expect("static selector");
    let tables = Selector::parse("table").expect("static selector");
    let compact = |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let unavailable = |e: scraper::ElementRef<'_>| {
        let text = e.text().collect::<String>();
        text.starts_with("[Image unavailable") && text.ends_with(". View original]")
    };
    let mut html = retained_dom.root_element().inner_html();
    let mut changed = false;
    for image in incoming_dom.select(&images) {
        if !image
            .value()
            .attr("src")
            .is_some_and(|s| s.starts_with("/api/media/") && s.len() == 47)
        {
            continue;
        }
        let source = image
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .find(|e| e.value().name() == "a" && e.value().attr("href").is_some());
        if let Some(source) = source {
            let href = source.value().attr("href").expect("source link");
            let target: Vec<_> = retained_dom
                .select(&links)
                .filter(|e| e.value().attr("href") == Some(href))
                .collect();
            let sources = incoming_dom
                .select(&links)
                .filter(|e| e.value().attr("href") == Some(href))
                .count();
            if target.len() != 1 || sources != 1 || source.select(&images).count() != 1 {
                continue;
            }
            let target = target[0];
            if target.select(&images).next().is_some()
                || (!compact(target.text().collect()).is_empty() && !unavailable(target))
            {
                continue;
            }
            let replacement = format!("<a href=\"{}\">{}</a>", escaped(href), image.html());
            html = html.replacen(&target.html(), &replacement, 1);
            if let Some(fallback) = target.next_sibling().and_then(scraper::ElementRef::wrap)
                && fallback.value().name() == "a"
                && fallback.value().attr("href") == Some(page)
                && unavailable(fallback)
            {
                html = html.replacen(&fallback.html(), "", 1)
            }
            changed = true;
            continue;
        }
        let Some(figure) = image
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .find(|e| e.value().name() == "figure")
        else {
            continue;
        };
        let Some(caption) = figure.select(&captions).next() else {
            continue;
        };
        let caption = compact(caption.text().collect());
        if caption.is_empty() || figure.select(&images).count() != 1 {
            continue;
        }
        let same = |f: scraper::ElementRef<'_>| {
            f.select(&captions)
                .next()
                .is_some_and(|c| compact(c.text().collect()) == caption)
        };
        let targets: Vec<_> = retained_dom.select(&figures).filter(|f| same(*f)).collect();
        if targets.len() != 1 || incoming_dom.select(&figures).filter(|f| same(*f)).count() != 1 {
            continue;
        }
        let target = targets[0];
        if target.select(&images).next().is_some() || target.select(&tables).next().is_some() {
            continue;
        }
        let Some(target_caption) = target.select(&captions).next() else {
            continue;
        };
        let mut replacement = target.html();
        for fallback in target.select(&links) {
            if fallback.value().attr("href") == Some(page) && unavailable(fallback) {
                replacement = replacement.replacen(&fallback.html(), "", 1)
            }
        }
        replacement = replacement.replacen(
            &target_caption.html(),
            &format!("{}{}", image.html(), target_caption.html()),
            1,
        );
        html = html.replacen(&target.html(), &replacement, 1);
        changed = true;
    }
    changed.then(|| extraction::clean_html(&html, page, false))
}
fn store_images(tx: &rusqlite::Transaction<'_>, job: &Job, media: &mut MediaCopy) -> Result<()> {
    for image in media.images.drain(..) {
        let id:String=tx.query_row("INSERT INTO article_media(id,user_id,article_id,source_url,sha256,content_type,body) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(user_id,article_id,source_url,sha256) DO UPDATE SET source_url=excluded.source_url RETURNING id",params![image.id,OWNER,job.target_id,image.source,image.digest,image.content_type,image.body],|r|r.get(0))?;
        if id != image.id {
            media.html = media.html.replace(&image.id, &id);
            if let Some(logo) = media.logo.as_mut() {
                *logo = logo.replace(&image.id, &id)
            }
        }
    }
    Ok(())
}
fn escaped(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn raster(bytes: &[u8]) -> Result<String> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= network::MAX_RESPONSE,
        "Image exceeds 5 MB"
    );
    let format = image::guess_format(bytes)?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Ico => "image/x-icon",
        _ => bail!("Unsupported raster image"),
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
async fn cache_media(
    fetcher: &dyn Fetcher,
    copy: &extraction::Extraction,
    page: &str,
) -> MediaCopy {
    let image_selector = Selector::parse("img").expect("static selector");
    let parsed = Html::parse_fragment(&copy.html);
    let nodes: Vec<(String, String, String)> = parsed
        .select(&image_selector)
        .map(|e| {
            (
                e.html(),
                e.value().attr("src").unwrap_or_default().into(),
                e.value().attr("alt").unwrap_or_default().into(),
            )
        })
        .collect();
    let mut sources: Vec<String> = copy.logo_sources.iter().take(2).cloned().collect();
    sources.extend(nodes.iter().map(|(_, s, _)| s.clone()));
    let start = std::time::Instant::now();
    let mut images = Vec::new();
    let mut paths = HashMap::new();
    let mut downloaded = 0usize;
    let mut attempts = 0usize;
    for source in sources {
        if paths.contains_key(&source) {
            continue;
        }
        paths.insert(source.clone(), None);
        if attempts >= 22
            || downloaded >= 20 * 1024 * 1024
            || start.elapsed() >= Duration::from_secs(50)
        {
            continue;
        }
        let Ok(url) = network::normalize_url(&source) else {
            continue;
        };
        attempts += 1;
        let remaining = Duration::from_secs(50).saturating_sub(start.elapsed());
        let Ok(Ok(resource)) =
            tokio::time::timeout(remaining, fetcher.fetch(url.as_str(), true)).await
        else {
            continue;
        };
        downloaded = downloaded.saturating_add(resource.body.len());
        if downloaded > 20 * 1024 * 1024 {
            continue;
        }
        let Ok(content_type) = raster(&resource.body) else {
            continue;
        };
        let id = uuid::Uuid::new_v4().to_string();
        paths.insert(source.clone(), Some(format!("/api/media/{id}")));
        images.push(CachedImage {
            id,
            source: url.to_string(),
            digest: format!("{:x}", Sha256::digest(&resource.body)),
            content_type,
            body: resource.body,
        });
    }
    let logo = copy
        .logo_sources
        .iter()
        .take(2)
        .find_map(|s| paths.get(s).cloned().flatten());
    let mut html = parsed.root_element().inner_html();
    let mut missing = 0;
    for (node, source, alt) in nodes {
        let replacement = if let Some(Some(path)) = paths.get(&source) {
            format!("<img src=\"{}\" alt=\"{}\">", escaped(path), escaped(&alt))
        } else {
            missing += 1;
            format!(
                "<a href=\"{}\">[Image unavailable{}. View original]</a>",
                escaped(page),
                if alt.is_empty() {
                    String::new()
                } else {
                    format!(": {}", escaped(&alt))
                }
            )
        };
        html = html.replacen(&node, &replacement, 1);
    }
    MediaCopy {
        html: extraction::clean_html(&html, page, false),
        logo,
        missing,
        images,
    }
}
fn persist_article(
    c: &mut Connection,
    lease: &WorkerLease,
    job: &Job,
    resource: &network::Resource,
    copy: &extraction::Extraction,
    mut media: MediaCopy,
) -> Result<Option<String>> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    job_owned(&tx, lease, job)?;
    let current: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT html,status,error FROM articles WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL",
            params![OWNER, job.target_id],
            |r| Ok((r.get(0)?, r.get(1)?,r.get(2)?)),
        )
        .optional()?;
    let Some((html, status, current_error)) = current else {
        tx.commit()?;
        return Ok(None);
    };
    let status = CopyStatus::parse(&status).map_err(anyhow::Error::msg)?;
    tx.execute("INSERT INTO native_article_captures(id,user_id,article_id,source_url,sha256,body) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING",params![uuid::Uuid::new_v4().to_string(),OWNER,job.target_id,resource.url.as_str(),copy.source_sha256,resource.body])?;
    let diagnostic = match extraction::retention_decision(&html, status, copy) {
        extraction::RetentionDecision::Keep {
            verified,
            diagnostic,
        } => {
            if verified {
                let mut restored = None;
                if merge_reading_images(&html, &media.html, resource.url.as_str()).is_some() {
                    store_images(&tx, job, &mut media)?;
                    restored = merge_reading_images(&html, &media.html, resource.url.as_str())
                }
                let clear_media = restored
                    .as_ref()
                    .is_some_and(|h| media.missing == 0 && !h.contains("[Image unavailable"));
                let visible_error = if clear_media {
                    None
                } else if status != CopyStatus::Saved {
                    let pattern=regex::Regex::new(r"\d+ image\(s\) could not be downloaded\. View the original for missing media\.").expect("static image diagnostic");
                    current_error
                        .as_ref()
                        .and_then(|e| pattern.find(e).map(|m| m.as_str().to_owned()))
                } else {
                    current_error.filter(|e| {
                        e != "The refreshed body was shorter; the existing saved copy was retained."
                    })
                };
                tx.execute("UPDATE articles SET status='saved',html=COALESCE(?3,html),word_count=CASE WHEN ?3 IS NULL THEN word_count ELSE ?4 END,excerpt=CASE WHEN ?3 IS NULL THEN excerpt ELSE ?5 END,error=?6 WHERE user_id=?1 AND id=?2",params![OWNER,job.target_id,restored,restored.as_ref().map(|h|extraction::word_count(h)),restored.as_ref().map(|h|extraction::text(h).chars().take(260).collect::<String>()),visible_error])?;
            }
            (if media.missing > 0 {
                Some(format!(
                    "{}{} image(s) could not be downloaded. View the original for missing media.",
                    copy.error
                        .as_ref()
                        .map(|e| format!("{e} "))
                        .unwrap_or_default(),
                    media.missing
                ))
            } else {
                copy.error.clone()
            })
            .or_else(|| (!verified).then_some(diagnostic))
        }
        extraction::RetentionDecision::Replace => {
            store_images(&tx, job, &mut media)?;
            let error = if media.missing > 0 {
                Some(format!(
                    "{}{} image(s) could not be downloaded. View the original for missing media.",
                    copy.error
                        .as_ref()
                        .map(|e| format!("{e} "))
                        .unwrap_or_default(),
                    media.missing
                ))
            } else {
                copy.error.clone()
            };
            let words = copy.word_count;
            let excerpt = &copy.excerpt;
            tx.execute("UPDATE articles SET title=CASE WHEN ?3='' THEN title ELSE ?3 END,author=?4,site=CASE WHEN ?5='' THEN site ELSE ?5 END,html=?6,word_count=?7,excerpt=?8,status=?9,error=?10,logo_url=COALESCE(?11,logo_url),published_at=COALESCE(published_at,?12) WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL",params![OWNER,job.target_id,copy.title,copy.author,copy.site,media.html,words,excerpt,copy.status.as_str(),error,media.logo,copy.published_at])?;
            None
        }
    };
    // Immutable edition image references survive refresh and deletion.
    tx.execute("DELETE FROM article_media AS media WHERE user_id=?1 AND article_id=?2 AND NOT EXISTS(SELECT 1 FROM articles a WHERE a.user_id=?1 AND a.id=?2 AND (instr(a.html,media.id)>0 OR instr(COALESCE(a.logo_url,''),media.id)>0)) AND NOT EXISTS(SELECT 1 FROM magazine_articles s WHERE s.user_id=?1 AND instr(s.html,media.id)>0)",params![OWNER,job.target_id])?;
    tx.commit()?;
    Ok(diagnostic)
}
fn persist_metadata(
    c: &mut Connection,
    lease: &WorkerLease,
    job: &Job,
    copy: &extraction::Extraction,
) -> Result<Option<String>> {
    let _reservation = library::reserve_write(c, 131_072)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    job_owned(&tx, lease, job)?;
    // Background link intake discards the publisher body and all media. A failed
    // or weaker refresh preserves existing useful metadata and reading state.
    tx.execute("UPDATE articles SET title=CASE WHEN ?3='' OR word_count>?6 THEN title ELSE ?3 END,author=COALESCE(author,?4),site=CASE WHEN ?5='' OR word_count>?6 THEN site ELSE ?5 END,excerpt=CASE WHEN word_count<=?6 OR excerpt='' THEN ?7 ELSE excerpt END,word_count=max(word_count,?6),status=CASE WHEN status='saved' OR (status='partial' AND ?8='failed') THEN status ELSE ?8 END,error=CASE WHEN status='saved' THEN error ELSE ?9 END,published_at=COALESCE(published_at,?10),html='',logo_url=NULL WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL",params![OWNER,job.target_id,copy.title.chars().take(1000).collect::<String>(),copy.author.as_ref().map(|a| a.chars().take(500).collect::<String>()),copy.site.chars().take(500).collect::<String>(),copy.word_count,copy.excerpt.chars().take(260).collect::<String>(),copy.status.as_str(),copy.error.as_ref().map(|e| e.chars().take(2000).collect::<String>()),copy.published_at])?;
    tx.commit()?;
    Ok(copy.error.clone())
}

pub async fn run_one(
    c: &mut Connection,
    lease: &WorkerLease,
    fetcher: &dyn Fetcher,
) -> Result<bool> {
    run_one_observing(c, lease, fetcher, &|| {}).await
}

async fn run_one_observing(
    c: &mut Connection,
    lease: &WorkerLease,
    fetcher: &dyn Fetcher,
    started: &dyn Fn(),
) -> Result<bool> {
    let Some(job) = claim_job(c, lease)? else {
        return Ok(false);
    };
    if matches!(job.kind, JobKind::Feed) {
        // The claim transaction has committed. Wake the read owner before the
        // network await so its summary can project the actual running job.
        started();
    }
    let result = run_claimed(c, lease, &job, fetcher).await;
    // A suspended request can finish before the actor's heartbeat is polled.
    // Renew the same owner before handling its result, while leaving every
    // publishing boundary fenced against expired or displaced ownership.
    renew_worker(c, lease)?;
    if let Err(error) = finish_job(c, lease, &job, result) {
        owned(c, lease)?;
        // Deletion/unsubscribe can remove a job during a download. A changed job
        // token also cancels this result without stopping the process owner.
        let still_owned:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE user_id=?1 AND id=?2 AND state='running' AND lease_token=?3 AND lease_until>now())",params![OWNER,job.id,job.token],|r|r.get(0))?;
        if still_owned {
            return Err(error);
        }
    }
    Ok(true)
}
async fn run_claimed(
    c: &mut Connection,
    lease: &WorkerLease,
    job: &Job,
    fetcher: &dyn Fetcher,
) -> Result<Option<String>> {
    match job.kind {
        JobKind::Feed => {
            let feed: Option<(String, bool)> = c
                .query_row(
                    "SELECT url,active FROM feeds WHERE user_id=?1 AND id=?2",
                    params![OWNER, job.target_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((url, active)) = feed else {
                return Ok(None);
            };
            let pending:Option<String>=c.query_row("SELECT id FROM feed_captures WHERE user_id=?1 AND feed_id=?2 AND processed_at IS NULL ORDER BY received_at,id LIMIT 1",params![OWNER,job.target_id],|r|r.get(0)).optional()?;
            let id = if let Some(id) = pending {
                id
            } else if active {
                if db_compat::is_lean(c)? {
                    let state: Option<(String,Option<String>,Option<String>)> = c.query_row("SELECT content_hash,etag,last_modified FROM native_feed_state WHERE user_id=?1 AND feed_id=?2",params![OWNER,job.target_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
                    let fetched = fetcher
                        .fetch_feed(
                            &url,
                            state.as_ref().and_then(|s| s.1.as_deref()),
                            state.as_ref().and_then(|s| s.2.as_deref()),
                        )
                        .await?;
                    let unchanged = fetched.resource.as_ref().is_none_or(|r| {
                        state
                            .as_ref()
                            .is_some_and(|s| s.0 == format!("{:x}", Sha256::digest(&r.body)))
                    });
                    if unchanged {
                        persist_feed_validators(c, lease, job, &fetched)?;
                        return Ok(None);
                    }
                    let resource = fetched
                        .resource
                        .as_ref()
                        .context("Missing changed feed body")?;
                    let xml = std::str::from_utf8(&resource.body)
                        .context("Feed must use UTF-8 encoding")?;
                    let id = persist_capture(c, lease, job, xml)?;
                    replay_capture(c, lease, job, &id)?;
                    persist_feed_validators(c, lease, job, &fetched)?;
                    return Ok(None);
                }
                let resource = fetcher.fetch(&url, false).await?;
                let xml =
                    std::str::from_utf8(&resource.body).context("Feed must use UTF-8 encoding")?;
                persist_capture(c, lease, job, xml)?
            } else {
                return Ok(None);
            };
            replay_capture(c, lease, job, &id)?;
            Ok(None)
        }
        JobKind::Article => {
            let url: Option<String> = c
                .query_row(
                    "SELECT url FROM articles WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL",
                    params![OWNER, job.target_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(url) = url else { return Ok(None) };
            let resource = fetcher.fetch(&url, false).await?;
            let source = std::str::from_utf8(&resource.body)
                .context("Publisher page must use UTF-8 encoding")?;
            let copy =
                extraction::extract(source, resource.url.as_str()).map_err(anyhow::Error::msg)?;
            if db_compat::is_lean(c)? {
                persist_metadata(c, lease, job, &copy)
            } else {
                let media = cache_media(fetcher, &copy, resource.url.as_str()).await;
                persist_article(c, lease, job, &resource, &copy, media)
            }
        }
    }
}
fn persist_feed_validators(
    c: &mut Connection,
    lease: &WorkerLease,
    job: &Job,
    fetched: &network::ConditionalResource,
) -> Result<()> {
    ensure!(
        fetched
            .etag
            .as_ref()
            .is_none_or(|v| v.len() <= 1024 && !v.contains(['\r', '\n']))
            && fetched
                .last_modified
                .as_ref()
                .is_none_or(|v| v.len() <= 1000),
        "Feed validator exceeds its bound"
    );
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    job_owned(&tx, lease, job)?;
    tx.execute("INSERT INTO native_feed_state(user_id,feed_id,etag,last_modified) VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,feed_id) DO UPDATE SET etag=excluded.etag,last_modified=excluded.last_modified",params![OWNER,job.target_id,fetched.etag,fetched.last_modified])?;
    tx.execute(
        "UPDATE feeds SET last_polled_at=now(),error=NULL WHERE user_id=?1 AND id=?2",
        params![OWNER, job.target_id],
    )?;
    tx.commit()?;
    Ok(())
}

fn emit(events: &SyncSender<Event>, wake: &Arc<dyn Fn() + Send + Sync>, event: Event) {
    match events.try_send(event) {
        Ok(()) => wake(),
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
    }
}
fn lock(config: &Config) -> Result<File> {
    let metadata = std::fs::symlink_metadata(&config.private_directory)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.permissions().mode() & 0o077 == 0,
        "Intake storage must be a private directory"
    );
    let path = config.private_directory.join("intake.lock");
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Invalid intake lock file"
        )
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    FileExt::try_lock_exclusive(&file).context("Another native intake worker is active")?;
    Ok(file)
}
pub fn spawn(
    config: Config,
    events: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Result<Handle> {
    match std::fs::symlink_metadata(&config.private_directory) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_dir(),
            "Intake storage must be a regular directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&config.private_directory)?;
        }
        Err(error) => return Err(error.into()),
    }
    // The configured native root owns this directory. Prepare it before an
    // enabled actor attempts to acquire intake.lock on a fresh compact store.
    std::fs::set_permissions(
        &config.private_directory,
        std::fs::Permissions::from_mode(0o700),
    )?;
    let (commands, mut rx) = mpsc::channel(32);
    let thread=thread::Builder::new().name("read-later-intake".into()).spawn(move||{
        let result=(||->Result<()>{let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;let mut c=db_compat::open(&config.library_path)?;initialize(&c)?;let mut heartbeat=db_compat::open(&config.library_path)?;
            runtime.block_on(async{
                let mut active:Option<(File,WorkerLease)>=None;
                loop{
                    if active.is_none(){match rx.recv().await{Some(Command::Enable(true))=>{
                        let attempt=(||->Result<(File,WorkerLease)>{let file=lock(&config)?;let lease=claim_worker(&mut c)?.context("Another process owns the intake lease")?;Ok((file,lease))})();
                        match attempt{Ok(owner)=>{active=Some(owner);emit(&events,&wake,Event::State{enabled:true,error:None})},Err(e)=>emit(&events,&wake,Event::State{enabled:false,error:Some(e.to_string())})}
                    },Some(Command::Quit)|None=>break,_=>{}}continue}
                    let lease=active.as_ref().expect("active owner").1.clone();
                    match renew_worker(&mut heartbeat,&lease).and_then(|_|schedule_feeds(&mut c,&lease)){
                        Ok(scheduled) if scheduled>0=>emit(&events,&wake,Event::LibraryChanged),
                        Ok(_)=>{},
                        Err(e)=>{if let Err(e)=resume_expired_worker(&mut heartbeat,&lease,e){let _=release_worker(&mut c,&lease);active=None;emit(&events,&wake,Event::State{enabled:false,error:Some(e.to_string())})}continue}
                    }
                    enum Next{Worked(Result<bool>),Command(Option<Command>),Lost(anyhow::Error)}
                    let next={
                        let started=||emit(&events,&wake,Event::LibraryChanged);
                        let work=run_one_observing(&mut c,&lease,&PublicFetcher,&started);tokio::pin!(work);
                        let mut pulse=tokio::time::interval(Duration::from_secs(5));pulse.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);pulse.tick().await;
                        loop{tokio::select!{result=&mut work=>break Next::Worked(result),command=rx.recv()=>match command { Some(Command::Wake | Command::Enable(true))=>{}, command=>break Next::Command(command) },_=pulse.tick()=>if let Err(e)=renew_worker(&mut heartbeat,&lease){break Next::Lost(e)}}}
                    };
                    // Dropping the future cancels an in-flight request before a
                    // disable/quit command can release its fencing token.
                    match next{
                        Next::Worked(Ok(true))=>emit(&events,&wake,Event::LibraryChanged),
                        Next::Worked(Ok(false))=>{tokio::select!{_=tokio::time::sleep(Duration::from_secs(5))=>{},command=rx.recv()=>match command{Some(Command::Enable(false))=>{let _=release_worker(&mut c,&lease);active=None;emit(&events,&wake,Event::State{enabled:false,error:None})},Some(Command::Quit)|None=>{let _=release_worker(&mut c,&lease);break},_=>{}}}},
                        Next::Worked(Err(e))|Next::Lost(e)=>{if let Err(e)=resume_expired_worker(&mut heartbeat,&lease,e){let _=release_worker(&mut c,&lease);active=None;emit(&events,&wake,Event::State{enabled:false,error:Some(e.to_string())})}},
                        Next::Command(Some(Command::Enable(false)))=>{release_worker(&mut c,&lease)?;active=None;emit(&events,&wake,Event::State{enabled:false,error:None})},
                        Next::Command(Some(Command::Quit)|None)=>{release_worker(&mut c,&lease)?;break},
                        Next::Command(_)=>{}
                    }
                }Ok::<_,anyhow::Error>(())
            })
        })();if let Err(e)=result{emit(&events,&wake,Event::State{enabled:false,error:Some(e.to_string())})}
    })?;
    Ok(Handle {
        commands,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod lease_resume_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn feed_claim_wakes_the_reader_before_network_and_projects_outcomes() {
        use std::cell::Cell;
        struct SyntheticFeed<'a> {
            notifications: &'a Cell<u32>,
            fail: bool,
        }
        #[async_trait::async_trait(?Send)]
        impl Fetcher for SyntheticFeed<'_> {
            async fn fetch(&self, url: &str, _image: bool) -> Result<network::Resource> {
                assert!(
                    self.notifications.get() > 0,
                    "feed claim must notify before acquisition"
                );
                if self.fail {
                    anyhow::bail!("Synthetic publisher failure");
                }
                Ok(network::Resource { url: url::Url::parse(url)?, body: b"<rss version='2.0'><channel><title>Sample</title><link>https://example.org</link><description>Synthetic feed</description></channel></rss>".to_vec(), content_type: "application/rss+xml".into() })
            }
        }
        let (mut connection, lease) = owner();
        let id = library::add_feed(&mut connection, "https://example.org/feed").unwrap();
        let notifications = Cell::new(0);
        let started = || notifications.set(notifications.get() + 1);
        assert_eq!(
            library::feeds(&connection).unwrap()[0].activity,
            crate::domain::FeedActivity::Queued
        );
        assert!(
            run_one_observing(
                &mut connection,
                &lease,
                &SyntheticFeed {
                    notifications: &notifications,
                    fail: false
                },
                &started
            )
            .await
            .unwrap()
        );
        assert_eq!(notifications.get(), 1);
        assert_eq!(
            library::feeds(&connection).unwrap()[0].activity,
            crate::domain::FeedActivity::Idle
        );
        library::update_feed(&mut connection, &id, crate::domain::FeedAction::Sync).unwrap();
        assert!(
            run_one_observing(
                &mut connection,
                &lease,
                &SyntheticFeed {
                    notifications: &notifications,
                    fail: true
                },
                &started
            )
            .await
            .unwrap()
        );
        let failed = library::feeds(&connection).unwrap().remove(0);
        assert_eq!(failed.activity, crate::domain::FeedActivity::Queued);
        assert_eq!(failed.error.as_deref(), Some("Synthetic publisher failure"));
        connection
            .execute(
                "UPDATE jobs SET attempts=4,run_at=now() WHERE kind='feed' AND target_id=?1",
                [&id.0],
            )
            .unwrap();
        assert!(
            run_one_observing(
                &mut connection,
                &lease,
                &SyntheticFeed {
                    notifications: &notifications,
                    fail: true
                },
                &started
            )
            .await
            .unwrap()
        );
        assert_eq!(notifications.get(), 3);
        assert_eq!(
            library::feeds(&connection).unwrap()[0].activity,
            crate::domain::FeedActivity::Idle
        );
        assert!(
            !run_one_observing(
                &mut connection,
                &lease,
                &SyntheticFeed {
                    notifications: &notifications,
                    fail: true
                },
                &started
            )
            .await
            .unwrap()
        );
        assert_eq!(notifications.get(), 3, "no wake without a new claim");
    }

    fn owner() -> (Connection, WorkerLease) {
        let mut c = Connection::open_in_memory().unwrap();
        db_compat::register(&c).unwrap();
        db_compat::initialize(&c).unwrap();
        initialize(&c).unwrap();
        let lease = claim_worker(&mut c).unwrap().unwrap();
        (c, lease)
    }

    #[test]
    fn actor_recovers_expiry_between_heartbeat_and_scheduling_or_claim() {
        for scheduling in [false, true] {
            let (mut c, lease) = owner();
            c.execute(
                "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'",
                [],
            )
            .unwrap();
            let error = if scheduling {
                schedule_feeds(&mut c, &lease).unwrap_err()
            } else {
                claim_job(&mut c, &lease).unwrap_err()
            };
            assert!(error.is::<WorkerLeaseExpired>());
            resume_expired_worker(&mut c, &lease, error).unwrap();
            owned(&c, &lease).unwrap();
            assert!(claim_job(&mut c, &lease).unwrap().is_none());
        }
    }

    #[test]
    fn actor_preserves_real_errors_and_rejects_displacement_during_resume() {
        let (mut c, lease) = owner();
        c.execute(
            "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        let error = anyhow::anyhow!("Storage limit reached");
        assert_eq!(
            resume_expired_worker(&mut c, &lease, error)
                .unwrap_err()
                .to_string(),
            "Storage limit reached"
        );
        let error = owned(&c, &lease).unwrap_err();
        assert!(error.is::<WorkerLeaseExpired>());
        c.execute("UPDATE worker_leader SET token='replacement-worker'", [])
            .unwrap();
        assert!(resume_expired_worker(&mut c, &lease, error).is_err());
        let error = owned(&c, &lease).unwrap_err();
        assert!(!error.is::<WorkerLeaseExpired>());
        assert!(resume_expired_worker(&mut c, &lease, error).is_err());

        let (mut c, lease) = owner();
        c.execute(
            "UPDATE worker_leader SET lease_until='2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        let error = owned(&c, &lease).unwrap_err();
        c.execute("UPDATE users SET disabled_at=now() WHERE id=?1", [OWNER])
            .unwrap();
        assert_eq!(
            resume_expired_worker(&mut c, &lease, error)
                .unwrap_err()
                .to_string(),
            "The local library is unavailable"
        );
    }
}
