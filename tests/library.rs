use macos_read_later::{db_compat, domain::*, library};
use rusqlite::params;
fn db() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    db_compat::register(&c).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    db_compat::initialize(&c).unwrap();
    c
}
#[test]
fn duplicate_arrivals_preserve_read_archive_and_tombstone() {
    let mut c = db();
    let (a, new) = library::save(
        &mut c,
        "https://example.com/a",
        ArrivalKind::Web,
        "save1",
        "Saved",
        None,
        None,
    )
    .unwrap();
    assert!(new);
    library::update(&c, &a, None, Some(true)).unwrap();
    let (b, new) = library::save(
        &mut c,
        "https://example.com/a#fragment",
        ArrivalKind::Rss,
        "feed:a",
        "Feed",
        None,
        None,
    )
    .unwrap();
    assert_eq!(a, b);
    assert!(!new);
    let body = library::body(&c, &a).unwrap();
    assert!(body.article.read_at.is_some() && body.article.archived_at.is_some());
    library::update(&c, &a, Some(false), None).unwrap();
    library::update(&c, &a, None, Some(false)).unwrap();
    assert!(library::body(&c, &a).unwrap().article.read_at.is_none());
    library::delete(&mut c, std::slice::from_ref(&a)).unwrap();
    let (_, new) = library::save(
        &mut c,
        "https://example.com/a",
        ArrivalKind::Rss,
        "feed:a",
        "Feed",
        None,
        None,
    )
    .unwrap();
    assert!(!new);
    assert!(library::body(&c, &a).is_err());
    let (restored, new) = library::save(
        &mut c,
        "https://example.com/a",
        ArrivalKind::Web,
        "save2",
        "Saved",
        None,
        None,
    )
    .unwrap();
    assert!(new);
    assert_eq!(a, restored);
    assert!(library::body(&c, &a).unwrap().article.read_at.is_none());
}
#[test]
fn query_searches_previews_groups_hosts_and_orders_missing_metadata_last() {
    let mut c = db();
    let (a, _) = library::save(
        &mut c,
        "https://www.example.com/a",
        ArrivalKind::Rss,
        "rss1",
        "Feed",
        Some("Alpha"),
        None,
    )
    .unwrap();
    let (b, _) = library::save(
        &mut c,
        "https://example.com/b",
        ArrivalKind::Web,
        "b",
        "Saved",
        Some("Beta"),
        None,
    )
    .unwrap();
    c.execute(
        "UPDATE articles SET html='<p>Unsearchable full body</p>',excerpt='Unique retained BODY',author='Zed' WHERE id=?1",
        [&a.0],
    )
    .unwrap();
    let mut q = Query::for_scope(Scope::Inbox);
    q.search = "retained body".into();
    assert_eq!(library::articles(&c, &q, 0).unwrap()[0].id, a);
    q.search.clear();
    q.publication = "example.com".into();
    assert_eq!(library::articles(&c, &q, 0).unwrap().len(), 2);
    q.origin = Origin::Saved;
    assert_eq!(library::articles(&c, &q, 0).unwrap()[0].id, b);
    q.origin = Origin::All;
    q.sort = Sort::Author;
    for ascending in [true, false] {
        q.ascending = ascending;
        assert_eq!(library::articles(&c, &q, 0).unwrap()[0].id, a);
    }
    assert_eq!(
        library::publications(&c, &q).unwrap(),
        vec![("example.com".into(), 2)]
    );
}
#[test]
fn finalized_snapshots_and_referenced_media_survive_deletion() {
    let mut c = db();
    let (a, _) = library::save(
        &mut c,
        "https://example.com/a",
        ArrivalKind::Web,
        "a",
        "Saved",
        None,
        None,
    )
    .unwrap();
    let media = uuid::Uuid::new_v4().to_string();
    let html = format!("<p>Original prose</p><img src='/api/media/{media}'>");
    c.execute(
        "UPDATE articles SET html=?2,status='saved',word_count=2 WHERE id=?1",
        params![a.0, html],
    )
    .unwrap();
    c.execute("INSERT INTO article_media(id,user_id,article_id,source_url,sha256,content_type,body) VALUES(?1,?2,?3,'https://example.com/img',?4,'image/png',?5)",params![media,OWNER,a.0,"a".repeat(64),[1u8,2,3].as_slice()]).unwrap();
    use macos_read_later::magazine::{self, Design, DraftInput, PaperSize};
    magazine::initialize(&c).unwrap();
    let issue = magazine::save_draft(
        &mut c,
        &DraftInput {
            id: None,
            title: "Issue".into(),
            subtitle: String::new(),
            paper_size: PaperSize::Letter,
            article_ids: vec![a.clone()],
            refresh_copies: false,
            design: Design::default(),
            presentation: Default::default(),
        },
    )
    .unwrap();
    c.execute(
        "UPDATE magazines SET finalized_at=now() WHERE id=?1",
        [&issue.0],
    )
    .unwrap();
    assert!(
        c.execute(
            "UPDATE magazine_articles SET html='Changed' WHERE magazine_id=?1",
            [&issue.0]
        )
        .is_err()
    );
    library::delete(&mut c, std::slice::from_ref(&a)).unwrap();
    let snapshot: String = c
        .query_row(
            "SELECT html FROM magazine_articles WHERE magazine_id=?1",
            [&issue.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(snapshot, html);
    let n: i64 = c
        .query_row("SELECT count(*) FROM article_media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}
#[test]
fn pagination_and_bulk_filter_count_are_stable() {
    let mut c = db();
    for i in 0..75 {
        library::save(
            &mut c,
            &format!("https://example.com/{i}"),
            ArrivalKind::Web,
            &i.to_string(),
            "Saved",
            None,
            None,
        )
        .unwrap();
    }
    let q = Query::for_scope(Scope::Inbox);
    let first = library::articles(&c, &q, 0).unwrap();
    let second = library::articles(&c, &q, 60).unwrap();
    assert_eq!(first.len(), 60);
    assert_eq!(second.len(), 15);
    assert!(!first.iter().any(|a| second.iter().any(|b| a.id == b.id)));
    assert!(library::delete_matching(&mut c, &q, 74).is_err());
    assert_eq!(library::counts(&c).unwrap().all, 75);
    assert_eq!(library::delete_matching(&mut c, &q, 75).unwrap(), 75);
}
