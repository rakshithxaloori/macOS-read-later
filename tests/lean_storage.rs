use macos_read_later::{
    db_compat,
    domain::CopyStatus,
    storage::{CACHE_BYTES, CachedArticle, CachedMedia, LIMIT_BYTES, Storage},
};
use std::fs;

fn article(bytes: usize) -> CachedArticle {
    CachedArticle {
        html: "<p>Complete article ending.</p>".into(),
        media: vec![CachedMedia {
            id: "image".into(),
            mime_type: "image/png".into(),
            bytes: vec![7; bytes],
        }],
        status: CopyStatus::Saved,
        error: None,
        word_count: 4,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[test]
fn every_owner_shares_live_reservations_and_recovers_crashed_claims() {
    let directory = tempfile::tempdir().unwrap();
    let one = Storage::new(directory.path(), 45_000_000).unwrap();
    let claim = one.reserve("production", 40_000_000).unwrap();
    let two = Storage::new(directory.path(), 45_000_000).unwrap();
    assert_eq!(two.snapshot().unwrap().reserved_bytes, 40_000_000);
    assert!(two.require(14_000_000).is_err());
    assert!(claim.check(40_000_001).is_err());
    drop(claim);
    assert_eq!(two.snapshot().unwrap().reserved_bytes, 0);
    fs::write(
        directory.path().join("storage-reservations/crashed"),
        br#"{"bytes":50000000,"kind":"crashed"}"#,
    )
    .unwrap();
    let restarted = Storage::new(directory.path(), 45_000_000).unwrap();
    assert_eq!(restarted.snapshot().unwrap().reserved_bytes, 0);
    assert!(
        !directory
            .path()
            .join("storage-reservations/crashed")
            .exists()
    );
}

#[test]
fn archive_invalidation_fences_a_late_download_across_owners() {
    let directory = tempfile::tempdir().unwrap();
    let one = Storage::new(directory.path(), 45_000_000).unwrap();
    let two = Storage::new(directory.path(), 45_000_000).unwrap();
    let epoch = one.cache_generation("article").unwrap();
    assert!(
        one.cache_put("article", epoch, false, article(100))
            .unwrap()
    );
    assert_eq!(
        two.cache_get("article").unwrap().unwrap().media[0]
            .bytes
            .len(),
        100
    );
    assert_eq!(two.invalidate("article").unwrap(), epoch + 1);
    assert!(two.cache_get("article").unwrap().is_none());
    assert!(
        !one.cache_put("article", epoch, false, article(100))
            .unwrap()
    );
    assert!(
        !one.cache_put("article", epoch + 1, true, article(100))
            .unwrap()
    );
    assert!(one.cache_get("article").unwrap().is_none());
}

#[test]
fn cache_is_bounded_and_quota_reclaims_it_before_durable_work() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::new(directory.path(), 45_000_000).unwrap();
    for n in 0..6 {
        assert!(
            storage
                .cache_put(&format!("a{n}"), 0, false, article(5_000_000))
                .unwrap()
        );
        assert!(storage.snapshot().unwrap().cache_bytes <= CACHE_BYTES);
    }
    assert!(storage.cache_get("a0").unwrap().is_none());
    assert!(storage.cache_get("a5").unwrap().is_some());
    let claim = storage.require(45_000_000).unwrap();
    assert!(storage.snapshot().unwrap().cache_bytes < 10_000_000);
    assert!(storage.snapshot().unwrap().total_bytes() < LIMIT_BYTES);
    claim.finish().unwrap();
}

#[test]
fn expiry_and_interrupted_cache_writes_are_reclaimed_at_restart() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::new(directory.path(), 45_000_000).unwrap();
    storage
        .cache_put("expired", 0, false, article(1000))
        .unwrap();
    let path = directory.path().join("article-cache-index.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["entries"]["expired"]["expires_at"] = serde_json::json!(0);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    fs::create_dir(directory.path().join("article-cache/interrupted")).unwrap();
    fs::write(
        directory.path().join("article-cache/interrupted/body"),
        vec![2; 10_000],
    )
    .unwrap();
    let storage = Storage::new(directory.path(), 45_000_000).unwrap();
    assert!(storage.cache_get("expired").unwrap().is_none());
    assert_eq!(storage.snapshot().unwrap().cache_bytes, 0);
}

#[test]
fn quotas_include_database_wal_work_and_actual_bundle_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::new(directory.path(), 40_000_000).unwrap();
    let baseline = storage.snapshot().unwrap().managed_bytes;
    for name in [
        "library.sqlite3",
        "library.sqlite3-wal",
        "work.pending",
        "credentials.sqlite3",
    ] {
        fs::write(directory.path().join(name), vec![0; 10_000]).unwrap();
    }
    assert!(storage.snapshot().unwrap().managed_bytes >= baseline + 40_000);
    storage.set_bundle_bytes(50_000_000).unwrap();
    assert_eq!(storage.snapshot().unwrap().bundle_bytes, 50_000_000);
    assert!(storage.require(50_000_000).is_err());
}

#[test]
fn lean_is_explicit_and_rejects_existing_payloads() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    db_compat::register(&c).unwrap();
    db_compat::initialize(&c).unwrap();
    assert!(!db_compat::is_lean(&c).unwrap());
    c.execute("INSERT INTO articles(id,user_id,url,title,site,html) VALUES('a',?1,'https://example.com/a','Article','Example','<p>Saved</p>')",[macos_read_later::domain::OWNER]).unwrap();
    assert!(db_compat::initialize_lean(&c).is_err());
    c.execute("UPDATE articles SET html=''", []).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    assert!(db_compat::is_lean(&c).unwrap());
    assert!(
        c.execute("UPDATE articles SET html='<p>Cannot persist this</p>'", [])
            .is_err()
    );
}

#[test]
fn reopened_lean_connections_reclaim_payload_pages_and_short_wal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite3");
    let c = db_compat::open(&path).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    c.execute_batch("CREATE TABLE temporary_payload(id INTEGER PRIMARY KEY,body BLOB);")
        .unwrap();
    c.execute(
        "INSERT INTO temporary_payload(body) VALUES(?1)",
        [vec![3u8; 1_000_000]],
    )
    .unwrap();
    let before = fs::metadata(&path).unwrap().len();
    c.execute("DELETE FROM temporary_payload", []).unwrap();
    db_compat::reclaim(&c).unwrap();
    let after = fs::metadata(&path).unwrap().len();
    assert!(after < before);
    assert_eq!(
        c.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(c);
    let c = db_compat::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row("PRAGMA wal_autocheckpoint", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        16
    );
    assert!(
        c.query_row("PRAGMA max_page_count", [], |r| r.get::<_, i64>(0))
            .unwrap()
            * 4096
            <= 26_500_000
    );
}

#[test]
fn read_only_audit_reports_the_installed_bundle_and_every_managed_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let bundle = directory.path().join("Read Later.app");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&bundle).unwrap();
    fs::create_dir(root.join("article-cache")).unwrap();
    fs::write(root.join("library.sqlite3"), vec![1; 1000]).unwrap();
    fs::write(root.join("library.sqlite3-wal"), vec![1; 2000]).unwrap();
    fs::write(root.join("article-cache/body"), vec![1; 3000]).unwrap();
    fs::write(bundle.join("executable"), vec![1; 4000]).unwrap();
    let report = macos_read_later::storage::audit(&root, &bundle).unwrap();
    assert_eq!(report.managed_breakdown.len(), 3);
    assert_eq!(
        report.managed_bytes,
        macos_read_later::storage::measured_bytes(&root).unwrap()
    );
    assert_eq!(
        report.total_bytes,
        report.managed_bytes + report.bundle_bytes
    );
    assert!(report.below_limit);
    assert!(!root.join("storage-policy.json").exists());
    assert!(macos_read_later::storage::audit(&root, &root).is_err());
}

#[test]
fn allocation_accounting_rejects_dangling_symlinks_and_ignores_socket_payloads() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("native.socket");
    let _listener = UnixListener::bind(&socket).unwrap();
    assert_eq!(
        macos_read_later::storage::measured_bytes(&socket).unwrap(),
        0
    );
    let link = directory.path().join("dangling");
    symlink(directory.path().join("missing"), &link).unwrap();
    assert!(macos_read_later::storage::measured_bytes(&link).is_err());
}
