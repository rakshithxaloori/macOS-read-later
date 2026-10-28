use macos_read_later::{
    db_compat,
    domain::{ArrivalKind, ArticleId, CopyStatus},
    library,
    magazine::*,
};
use std::collections::HashMap;
fn fixture() -> (tempfile::TempDir, rusqlite::Connection, ArticleId) {
    let directory = tempfile::tempdir().unwrap();
    let mut c = db_compat::open(&directory.path().join("library.sqlite3")).unwrap();
    db_compat::initialize_lean(&c).unwrap();
    initialize(&c).unwrap();
    let (id, _) = library::save(
        &mut c,
        "https://example.com/engineering",
        ArrivalKind::Web,
        "lean-magazine",
        "Saved",
        Some("Engineering materials"),
        None,
    )
    .unwrap();
    c.execute(
        "UPDATE articles SET status='saved',word_count=1800 WHERE id=?1",
        [&id.0],
    )
    .unwrap();
    (directory, c, id)
}
fn input(id: ArticleId) -> DraftInput {
    DraftInput {
        id: None,
        title: "Lean issue".into(),
        subtitle: "Complete articles".into(),
        paper_size: PaperSize::Letter,
        article_ids: vec![id],
        refresh_copies: false,
        design: Design::default(),
        presentation: Default::default(),
    }
}
fn png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(8, 12, image::Rgba([20, 35, 50, 255]));
    let mut buffer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}
fn prepared(id: ArticleId) -> PreparedArticle {
    PreparedArticle{snapshot:Snapshot{article_id:id,position:0,title:"Engineering materials".into(),url:"https://example.com/engineering".into(),author:None,html:"<p>A complete retained draft.</p><img src='https://example.com/image.png'><p>The exact ending.</p>".into(),presented_html:String::new(),word_count:8,content_status:CopyStatus::Saved,content_warning:None,presentation:Presentation::default()},media:HashMap::from([("https://example.com/image.png".into(),png())]),source_sha256:"a".repeat(64),retrieved_at:"2026-10-09T10:00:00Z".into()}
}
#[test]
fn uncached_metadata_is_eligible_and_snapshots_require_preparation() {
    let (_directory, mut c, id) = fixture();
    assert_eq!(plans(&c).unwrap().eligible_count, 1);
    c.execute(
        "UPDATE articles SET status='partial',word_count=0 WHERE id=?1",
        [&id.0],
    )
    .unwrap();
    assert_eq!(
        plans(&c).unwrap().eligible_count,
        1,
        "RSS previews need publisher acquisition before completeness can be decided"
    );
    assert!(
        save_draft(&mut c, &input(id))
            .unwrap_err()
            .to_string()
            .contains("Acquire")
    );
}
#[test]
fn production_owns_images_independently_of_archive_and_cache_eviction() {
    let (_directory, mut c, id) = fixture();
    let draft = save_prepared_draft(&mut c, &input(id.clone()), &[prepared(id.clone())]).unwrap();
    c.execute(
        "UPDATE articles SET archived_at=now(),read_at=now() WHERE id=?1",
        [&id.0],
    )
    .unwrap();
    macos_read_later::storage::Storage::for_database(&c)
        .unwrap()
        .invalidate(&id.0)
        .unwrap();
    let snapshot = load_issue(&c, &draft).unwrap();
    assert!(snapshot.articles[0].html.contains("The exact ending."));
    let body: Vec<u8> = c
        .query_row(
            "SELECT body FROM native_magazine_media WHERE magazine_id=?1",
            [&draft.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body, png());
    assert_eq!(
        c.query_row("SELECT html FROM articles WHERE id=?1", [&id.0], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        ""
    );
    delete_draft(&mut c, &draft).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM native_magazine_media", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        0
    );
}
#[test]
fn oversized_owned_snapshots_are_refused_before_writing() {
    let (_directory, mut c, id) = fixture();
    let mut copy = prepared(id.clone());
    copy.media.insert("huge".into(), vec![0; 35_000_000]);
    let error = save_prepared_draft(&mut c, &input(id), &[copy]).unwrap_err();
    assert!(error.to_string().contains("cannot fit") || error.to_string().contains("budget"));
    assert_eq!(
        c.query_row("SELECT count(*) FROM magazines", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn editing_keeps_frozen_images_and_refresh_replaces_them() {
    let (_directory, mut c, article) = fixture();
    let mut draft_input = input(article.clone());
    let original = prepared(article.clone());
    let id = save_prepared_draft(&mut c, &draft_input, std::slice::from_ref(&original)).unwrap();
    draft_input.id = Some(id.clone());
    draft_input.title = "Edited title".into();
    save_prepared_draft(&mut c, &draft_input, &[]).unwrap();
    let retained = load_issue(&c, &id).unwrap();
    assert_eq!(retained.title, "Edited title");
    assert_eq!(retained.articles[0].html, original.snapshot.html);
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM native_magazine_media WHERE magazine_id=?1",
            [&id.0],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        1
    );

    let mut refreshed = prepared(article);
    refreshed.snapshot.html = "<p>A complete newer copy without the former image.</p>".into();
    refreshed.media.clear();
    refreshed.source_sha256 = "b".repeat(64);
    draft_input.refresh_copies = true;
    save_prepared_draft(&mut c, &draft_input, std::slice::from_ref(&refreshed)).unwrap();
    assert_eq!(
        load_issue(&c, &id).unwrap().articles[0].html,
        refreshed.snapshot.html
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM native_magazine_media WHERE magazine_id=?1",
            [&id.0],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0,
        "Explicit refresh retires unreferenced production images"
    );
    assert_eq!(
        c.query_row(
            "SELECT source_sha256 FROM native_magazine_acquisitions WHERE magazine_id=?1",
            [&id.0],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        refreshed.source_sha256
    );
}

#[test]
fn imported_viewer_cannot_be_exported_and_removed_or_reclassified_as_native() {
    let (_directory, mut c, _article) = fixture();
    let issue = macos_read_later::domain::MagazineId::default();
    let pdf = b"%PDF-1.4 imported viewer fixture";
    let sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(pdf));
    c.execute(
        "INSERT INTO magazines(id,user_id,title,subtitle,paper_size,design) VALUES(?1,?2,'Imported','Preserved','Letter',?3)",
        rusqlite::params![issue.0, macos_read_later::domain::OWNER, serde_json::to_string(&Design::default()).unwrap()],
    ).unwrap();
    c.execute("INSERT INTO magazine_pdfs(user_id,magazine_id,body,sha256,page_count,paper_size,renderer_version,verification) VALUES(?1,?2,?3,?4,2,'Letter','original','{}')",rusqlite::params![macos_read_later::domain::OWNER,issue.0,pdf,sha]).unwrap();
    c.execute("INSERT INTO native_viewer_editions(magazine_id,sha256,page_count,provenance_sha256) VALUES(?1,?2,2,?3)",rusqlite::params![issue.0,sha,"a".repeat(64)]).unwrap();
    c.execute(
        "UPDATE magazines SET finalized_at=now() WHERE id=?1",
        [&issue.0],
    )
    .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("imported.pdf");
    assert!(!edition(&c, &issue).unwrap().can_export_remove_pdf);
    assert!(
        export_and_remove_pdf(&mut c, &issue, &sha, &path)
            .unwrap_err()
            .to_string()
            .contains("protected")
    );
    assert!(!path.exists());
    assert!(c.execute("INSERT INTO native_pdf_exports(magazine_id,sha256,page_count,bytes,path) VALUES(?1,?2,2,?3,'forged.pdf')",rusqlite::params![issue.0,sha,pdf.len() as i64]).is_err());
    assert!(c.execute("INSERT INTO native_viewer_provenance(magazine_id,provenance) VALUES(?1,'{\"origin\":\"native-finalization-v1\",\"retention\":\"viewer-pdf-only\"}')",[&issue.0]).is_err());
    assert!(
        c.execute("DELETE FROM magazine_pdfs WHERE magazine_id=?1", [&issue.0])
            .is_err()
    );
    assert_eq!(
        c.query_row(
            "SELECT body FROM magazine_pdfs WHERE magazine_id=?1",
            [&issue.0],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        pdf
    );
}
