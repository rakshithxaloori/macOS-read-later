use macos_read_later::{
    db_compat,
    domain::{CopyStatus, OWNER},
    imports,
    storage::{CachedArticle, Storage},
};

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
fn removing_staged_run_preserves_other_run_and_cache() {
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
    // The disposable cache is no longer evicted to make room.
    assert!(storage.cache_get("cached").unwrap().is_some());
}
