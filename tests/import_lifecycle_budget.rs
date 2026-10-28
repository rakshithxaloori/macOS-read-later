use macos_read_later::{
    db_compat,
    domain::{CopyStatus, OWNER},
    imports,
    storage::{CachedArticle, LIMIT_BYTES, SAFETY_BYTES, Storage},
};
use rusqlite::params;

fn staged(c: &mut rusqlite::Connection) -> String {
    let upload = tempfile::tempdir().unwrap();
    let path = upload.path().join("rows.csv");
    let mut csv = "url,title\n".to_owned();
    for i in 0..100 {
        csv.push_str(&format!(
            "https://example.org/{i},{}\n",
            "Title ".repeat(150)
        ));
    }
    std::fs::write(&path, csv).unwrap();
    imports::stage(c, &path).unwrap()
}

#[test]
fn removing_staged_run_evicts_cache_and_preserves_other_run_under_tight_budget() {
    let directory = tempfile::tempdir().unwrap();
    let mut c = db_compat::open(&directory.path().join("library.sqlite3")).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    let run = staged(&mut c);
    c.execute("INSERT INTO import_runs(id,user_id,fingerprint,filename,format,total) VALUES('other',?1,'other','other.csv','csv',0)", [OWNER]).unwrap();
    db_compat::reclaim(&c).unwrap();
    let storage = Storage::for_database(&c).unwrap();
    let copy = CachedArticle {
        html: "Disposable cache ".repeat(100_000),
        media: vec![],
        status: CopyStatus::Saved,
        error: None,
        word_count: 100_000,
        fetched_at: "2026-10-17T00:00:00Z".into(),
    };
    assert!(storage.cache_put("cached", 0, false, copy).unwrap());
    let before = storage.snapshot().unwrap();
    storage
        .set_bundle_bytes(LIMIT_BYTES - before.managed_bytes - SAFETY_BYTES - 65_536)
        .unwrap();
    imports::remove(&c, &run).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM import_rows WHERE run_id=?1",
            [&run],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM import_runs WHERE id='other'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let after = storage.snapshot().unwrap();
    assert!(after.cache_bytes < before.cache_bytes);
    assert!(after.total_bytes() < LIMIT_BYTES);
}

#[test]
fn retry_refusal_is_atomic_and_successful_rows_survive_later_retry() {
    let directory = tempfile::tempdir().unwrap();
    let mut c = db_compat::open(&directory.path().join("library.sqlite3")).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    let run = staged(&mut c);
    c.execute("UPDATE import_runs SET state='failed' WHERE id=?1", [&run])
        .unwrap();
    c.execute("UPDATE import_rows SET state=CASE WHEN position=1 THEN 'added' ELSE 'failed' END,error=CASE WHEN position=1 THEN NULL ELSE 'Retryable synthetic error' END WHERE run_id=?1", [&run]).unwrap();
    db_compat::reclaim(&c).unwrap();
    let storage = Storage::for_database(&c).unwrap();
    let usage = storage.snapshot().unwrap();
    storage
        .set_bundle_bytes(LIMIT_BYTES - usage.managed_bytes - SAFETY_BYTES - 16_384)
        .unwrap();
    assert!(imports::start(&mut c, &run, true).is_err());
    assert_eq!(
        c.query_row("SELECT state FROM import_runs WHERE id=?1", [&run], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "failed"
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM import_rows WHERE run_id=?1 AND state='failed'",
            [&run],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        99
    );
    storage.set_bundle_bytes(45_000_000).unwrap();
    imports::start(&mut c, &run, true).unwrap();
    let states: (i64, i64) = c.query_row("SELECT count(*) FILTER(WHERE state='added'),count(*) FILTER(WHERE state='pending') FROM import_rows WHERE run_id=?1 AND user_id=?2", params![run, OWNER], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(states, (1, 99));
    assert!(storage.snapshot().unwrap().total_bytes() < LIMIT_BYTES);
}
