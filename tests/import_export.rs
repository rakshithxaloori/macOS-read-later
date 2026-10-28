use macos_read_later::{db_compat, domain::*, exports, imports, library};
use serde_json::json;
fn db() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    db_compat::register(&c).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    db_compat::initialize(&c).unwrap();
    c
}
#[test]
fn supported_formats_preview_and_bounds() {
    let cases=[("urls.csv",b"url,title,read\nhttps://example.com/a,Article,true\n".as_slice()),("subscriptions.opml",b"<opml><body><outline text='Folder'><outline xmlUrl='https://example.com/feed' title='News'/></outline></body></opml>"),("bookmarks.html",b"<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><DT><A HREF='https://example.com/a' ADD_DATE='1700000000'>Article</A></DL>")];
    for (name, bytes) in cases {
        let preview = imports::preview(bytes, name).unwrap();
        assert_eq!(preview.items.len(), 1, "{name}");
        assert!(preview.problems.is_empty());
    }
    let invalid = imports::preview(b"url,title\nhttp://127.0.0.1/a,Private\n", "urls.csv").unwrap();
    assert_eq!(invalid.problems.len(), 1);
    assert!(invalid.items.is_empty());
    assert!(imports::preview(&vec![0; 20 * 1024 * 1024 + 1], "a.csv").is_err());
    assert!(imports::preview(b"<!DOCTYPE opml [<!ENTITY x 'x'>]><opml/>", "a.opml").is_err());
}
#[test]
fn durable_batches_replay_preserve_state_and_subscription_pause() {
    let mut c = db();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("portable.json");
    let input = json!({"format":"reading-list-v1","articles":[{"url":"https://example.com/a","title":"Original","html":"<p>Retained longer saved prose words here.</p>","status":"saved","read":false}],"feeds":[{"url":"https://example.com/rss","title":"News","active":true}]});
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let run = imports::stage(&mut c, &path).unwrap();
    assert_eq!(imports::stage(&mut c, &path).unwrap(), run);
    imports::start(&mut c, &run, false).unwrap();
    while imports::run_batch(&mut c).unwrap() {}
    let a = library::articles(&c, &Query::for_scope(Scope::Inbox), 0).unwrap()[0]
        .id
        .clone();
    library::update(&c, &a, None, Some(true)).unwrap();
    let f = library::feeds(&c).unwrap()[0].id.clone();
    library::update_feed(&mut c, &f, FeedAction::Pause).unwrap();
    let path2 = temp.path().join("second.json");
    let second = json!({"format":"reading-list-v1","articles":[{"url":"https://example.com/a","title":"Weaker","html":"<p>short</p>","read":false}],"feeds":[{"url":"https://example.com/rss","title":"News","active":true}]});
    std::fs::write(&path2, serde_json::to_vec(&second).unwrap()).unwrap();
    let run2 = imports::stage(&mut c, &path2).unwrap();
    imports::start(&mut c, &run2, false).unwrap();
    while imports::run_batch(&mut c).unwrap() {}
    let body = library::body(&c, &a).unwrap();
    assert!(body.article.read_at.is_some() && body.article.archived_at.is_some());
    assert_eq!(body.article.title, "Original");
    assert!(body.html.contains("Retained longer"));
    assert!(!library::feeds(&c).unwrap()[0].active);
    assert_eq!(imports::list(&c).unwrap()[0].duplicates, 2);
    library::delete(&mut c, std::slice::from_ref(&a)).unwrap();
    let third = temp.path().join("third.csv");
    std::fs::write(&third, "url,title\nhttps://example.com/a,Restore attempt\n").unwrap();
    let run3 = imports::stage(&mut c, &third).unwrap();
    imports::start(&mut c, &run3, false).unwrap();
    while imports::run_batch(&mut c).unwrap() {}
    assert!(library::body(&c, &a).is_err());
}
#[test]
fn export_excludes_credentials_and_retains_content_artifacts() {
    let mut c = db();
    let (a, _) = library::save(
        &mut c,
        "https://example.com/a",
        ArrivalKind::Web,
        "save",
        "Saved",
        None,
        None,
    )
    .unwrap();
    c.execute("UPDATE articles SET html='<h2>Retained</h2><p>Exact content.</p>',status='saved',word_count=4 WHERE id=?1",[&a.0]).unwrap();
    c.execute(
        "INSERT INTO whatsapp_keys(user_id,key,value) VALUES(?1,'secret-key','PRIVATE-CREDENTIAL')",
        [OWNER],
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("export.zip");
    exports::export(&c, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE-CREDENTIAL"));
    let preview = imports::preview(&bytes, "export.zip").unwrap();
    assert_eq!(preview.items.len(), 1);
    match &preview.items[0] {
        imports::Item::Article(a) => assert!(a.html.contains("Exact content.")),
        _ => panic!("Expected article"),
    };
    assert!(exports::export(&c, &path).is_err());
}

#[test]
fn import_batches_and_failed_retry_survive_replay() {
    let mut c = db();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("many.csv");
    let mut csv = "url,title\n".to_owned();
    for i in 0..56 {
        csv.push_str(&format!("https://example.com/{i},Article{i}\n"));
    }
    std::fs::write(&path, csv).unwrap();
    let run = imports::stage(&mut c, &path).unwrap();
    imports::start(&mut c, &run, false).unwrap();
    c.execute(
        "UPDATE import_rows SET item='{}' WHERE run_id=?1 AND position=1",
        [&run],
    )
    .unwrap();
    assert!(imports::run_batch(&mut c).unwrap());
    let p = imports::list(&c).unwrap();
    assert_eq!((p[0].added, p[0].failed, p[0].pending), (24, 1, 31));
    assert_eq!(p[0].row_errors.len(), 1);
    while imports::run_batch(&mut c).unwrap() {}
    assert_eq!(imports::list(&c).unwrap()[0].added, 55);
    let item = imports::preview(
        b"url,title\nhttps://example.com/retried,Retried\n",
        "urls.csv",
    )
    .unwrap()
    .items
    .remove(0);
    c.execute(
        "UPDATE import_rows SET item=?2 WHERE run_id=?1 AND position=1",
        rusqlite::params![run, serde_json::to_string(&item).unwrap()],
    )
    .unwrap();
    imports::start(&mut c, &run, true).unwrap();
    while imports::run_batch(&mut c).unwrap() {}
    assert_eq!(imports::list(&c).unwrap()[0].added, 56);
    assert_eq!(library::counts(&c).unwrap().all, 56);
}

#[test]
fn zip_traversal_and_crc_corruption_are_rejected() {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("../reading-list.json", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"{}").unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert!(imports::preview(&bytes, "invalid.zip").is_err());
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "reading-list.json",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(b"{\"format\":\"reading-list-v1\",\"articles\":[]}")
        .unwrap();
    let mut bytes = zip.finish().unwrap().into_inner();
    let i = bytes.windows(9).position(|w| w == b"articles\"").unwrap();
    bytes[i] = b'A';
    assert!(imports::preview(&bytes, "corrupt.zip").is_err());
}
