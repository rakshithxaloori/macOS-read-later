use macos_read_later::{
    db_compat,
    domain::{ArrivalKind, ArticleId, OWNER},
    library,
    magazine::*,
};
use rusqlite::params;
fn db() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    db_compat::register(&c).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    db_compat::initialize(&c).unwrap();
    initialize(&c).unwrap();
    c
}
fn article(c: &mut rusqlite::Connection, n: usize, host: &str, words: u32) -> ArticleId {
    let (id, _) = library::save(
        c,
        &format!("https://{host}/{n}"),
        ArrivalKind::Web,
        &n.to_string(),
        "Saved",
        Some(&format!("Engineering materials {n}")),
        None,
    )
    .unwrap();
    c.execute(
        "UPDATE articles SET html=?2,word_count=?3,status='saved' WHERE id=?1",
        params![
            id.0,
            format!("<p>Unique original paragraph {n}.</p><p>Complete ending {n}.</p>"),
            words
        ],
    )
    .unwrap();
    id
}
fn input(ids: Vec<ArticleId>) -> DraftInput {
    DraftInput {
        id: None,
        title: "An issue".into(),
        subtitle: "Synthetic fixtures".into(),
        paper_size: PaperSize::Letter,
        article_ids: ids,
        refresh_copies: false,
        design: Design::default(),
        presentation: Default::default(),
    }
}
fn png(color: u8) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(8, 12, image::Rgba([color, 35, 50, 255]));
    let mut writer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut writer, image::ImageFormat::Png)
        .unwrap();
    writer.into_inner()
}
#[test]
fn proposals_query_the_entire_eligible_library_and_are_deterministic() {
    let mut c = db();
    let mut ids = Vec::new();
    for n in 0..80 {
        ids.push(article(&mut c, n, &format!("p{}.example", n % 20), 1600));
    }
    c.execute("UPDATE articles SET read_at=now() WHERE id=?1", [&ids[0].0])
        .unwrap();
    c.execute(
        "UPDATE articles SET archived_at=now() WHERE id=?1",
        [&ids[1].0],
    )
    .unwrap();
    c.execute(
        "UPDATE articles SET deleted_at=now() WHERE id=?1",
        [&ids[2].0],
    )
    .unwrap();
    c.execute(
        "UPDATE articles SET status='partial' WHERE id=?1",
        [&ids[3].0],
    )
    .unwrap();
    exclude(&c, &ids[4], true).unwrap();
    save_draft(&mut c, &input(vec![ids[5].clone()])).unwrap();
    let a = plans(&c).unwrap();
    let b = plans(&c).unwrap();
    assert_eq!(a.eligible_count, 74);
    assert_eq!(
        serde_json::to_value(a.clone()).unwrap(),
        serde_json::to_value(b).unwrap()
    );
    for p in a.proposals {
        assert!((3..=13).contains(&p.articles.len()));
        let mut hosts = std::collections::HashMap::<String, usize>::new();
        for article in p.articles {
            *hosts
                .entry(
                    url::Url::parse(&article.url)
                        .unwrap()
                        .host_str()
                        .unwrap()
                        .into(),
                )
                .or_default() += 1;
        }
        assert!(hosts.values().all(|n| *n <= 2));
    }
}
#[test]
fn snapshots_reorder_without_refresh_and_only_explicit_refresh_uses_new_content() {
    let mut c = db();
    let a = article(&mut c, 1, "a.example", 1000);
    let b = article(&mut c, 2, "b.example", 1000);
    let mut draft = input(vec![a.clone(), b.clone()]);
    let id = save_draft(&mut c, &draft).unwrap();
    c.execute(
        "UPDATE articles SET html='<p>Later extraction.</p>' WHERE id=?1",
        [&a.0],
    )
    .unwrap();
    draft.id = Some(id.clone());
    draft.article_ids.reverse();
    save_draft(&mut c, &draft).unwrap();
    let issue = load_issue(&c, &id).unwrap();
    assert_eq!(issue.articles[0].article_id, b);
    assert!(issue.articles[1].html.contains("Unique original"));
    draft.refresh_copies = true;
    save_draft(&mut c, &draft).unwrap();
    assert_eq!(
        load_issue(&c, &id).unwrap().articles[1].html,
        "<p>Later extraction.</p>"
    );
}
#[test]
fn empty_pending_and_unacknowledged_partial_copies_block_production() {
    let mut c = db();
    let a = article(&mut c, 1, "a.example", 1000);
    let id = save_draft(&mut c, &input(vec![a.clone()])).unwrap();
    c.execute(
        "UPDATE magazine_articles SET content_status='pending' WHERE magazine_id=?1",
        [&id.0],
    )
    .unwrap();
    assert!(queue(&mut c, &id, true).is_err());
    c.execute(
        "UPDATE magazine_articles SET content_status='partial' WHERE magazine_id=?1",
        [&id.0],
    )
    .unwrap();
    assert!(queue(&mut c, &id, false).is_err());
    queue(&mut c, &id, true).unwrap();
    assert_eq!(
        edition(&c, &id).unwrap().production.unwrap().state,
        ProductionState::Queued
    );
    assert!(
        save_draft(
            &mut c,
            &DraftInput {
                id: Some(id.clone()),
                ..input(vec![a])
            }
        )
        .is_err()
    );
}
#[test]
fn generated_originals_are_immutable_and_stale_requests_cannot_publish() {
    let mut c = db();
    let a = article(&mut c, 1, "a.example", 1000);
    let mut draft = input(vec![a]);
    let id = save_draft(&mut c, &draft).unwrap();
    let old = request_covers(&mut c, &id).unwrap();
    draft.id = Some(id.clone());
    draft.title = "Changed issue".into();
    save_draft(&mut c, &draft).unwrap();
    let obsolete = CoverFulfilment {
        magazine_id: id.clone(),
        request_id: old.id,
        input_sha256: old.input_sha256,
        front: png(120),
        back: png(130),
        note: "Codex generated pair".into(),
    };
    assert!(fulfil_covers(&mut c, &obsolete).is_err());
    let request = request_covers(&mut c, &id).unwrap();
    let correct = CoverFulfilment {
        request_id: request.id.clone(),
        input_sha256: request.input_sha256.clone(),
        ..obsolete
    };
    let completed = fulfil_covers(&mut c, &correct).unwrap();
    assert_eq!(completed.state, "ready");
    assert_eq!(
        fulfil_covers(&mut c, &correct).unwrap().front_asset,
        completed.front_asset
    );
    let wrong = CoverFulfilment {
        front: png(200),
        ..correct.clone()
    };
    assert!(fulfil_covers(&mut c, &wrong).is_err());
    let originals: (Vec<u8>, Vec<u8>) = c
        .query_row(
            "SELECT original_front,original_back FROM magazine_cover_requests WHERE id=?1",
            [&request.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(originals, (correct.front, correct.back));
    assert!(
        c.execute(
            "UPDATE magazine_artwork SET label='changed' WHERE magazine_id=?1",
            [&id.0]
        )
        .is_err()
    );
}
#[test]
fn source_repairs_require_unique_anchors_original_bytes_and_matching_provenance() {
    let mut c = db();
    let a = article(&mut c, 1, "a.example", 1000);
    let mut draft = input(vec![a.clone()]);
    let id = save_draft(&mut c, &draft).unwrap();
    let repair = RepairInput {
        context: None,
        magazine_id: id.clone(),
        body: png(20),
        label: "Recovered source chart".into(),
        source_url: "https://a.example/1".into(),
        image_url: "https://a.example/chart.png".into(),
        retrieved_at: "2026-10-08T00:00:00Z".into(),
    };
    let asset = add_repair(&mut c, &repair).unwrap();
    draft.id = Some(id.clone());
    draft.presentation.insert(
        a.clone(),
        Presentation {
            insertions: vec![ImageInsertion {
                asset_id: asset.id.clone(),
                sha256: asset.sha256.clone(),
                after_text: "Unique original paragraph 1.".into(),
                alt: "Original chart".into(),
                note: "Source chart".into(),
                source_url: asset.source_url.clone(),
                image_url: asset.image_url.clone(),
                retrieved_at: asset.retrieved_at.clone(),
            }],
            ..Default::default()
        },
    );
    save_draft(&mut c, &draft).unwrap();
    let original = load_issue(&c, &id).unwrap().articles[0].html.clone();
    assert!(!original.contains("Original chart"));
    let body: Vec<u8> = c
        .query_row(
            "SELECT original_body FROM native_magazine_source_repairs WHERE asset_id=?1",
            [&asset.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body, repair.body);
    draft.presentation.get_mut(&a).unwrap().insertions[0].source_url =
        "https://different.example".into();
    assert!(save_draft(&mut c, &draft).is_err());
    draft.presentation.get_mut(&a).unwrap().insertions[0].source_url = asset.source_url;
    draft.presentation.get_mut(&a).unwrap().insertions[0].after_text = "Missing anchor".into();
    assert!(save_draft(&mut c, &draft).is_err());
}
#[test]
fn finalize_requires_current_proof_exact_review_and_never_changes_read_state() {
    let mut c = db();
    let a = article(&mut c, 1, "a.example", 1000);
    let id = save_draft(&mut c, &input(vec![a.clone()])).unwrap();
    assert!(finalize(&mut c, &id).is_err());
    queue(&mut c, &id, false).unwrap();
    assert!(finalize(&mut c, &id).is_err());
    c.execute(
        "UPDATE magazine_production SET state='done' WHERE magazine_id=?1",
        [&id.0],
    )
    .unwrap();
    let hash: String = c
        .query_row(
            "SELECT input_sha256 FROM native_magazine_jobs WHERE magazine_id=?1",
            [&id.0],
            |r| r.get(0),
        )
        .unwrap();
    let pdf = b"%PDF-1.4 synthetic test bytes";
    use sha2::{Digest, Sha256};
    let sha = format!("{:x}", Sha256::digest(pdf));
    let proof = serde_json::json!({"sha256":sha,"input_sha256":hash,"complete_text":true,"prose_endings":true,"geometry":true,"article_openings":true,"links":true,"cover_copy":true});
    c.execute("INSERT INTO magazine_pdfs(user_id,magazine_id,body,sha256,page_count,paper_size,renderer_version,verification) VALUES(?1,?2,?3,?4,2,'Letter','synthetic-test',?5)",params![OWNER,id.0,pdf.as_slice(),sha,proof.to_string()]).unwrap();
    c.execute("INSERT INTO native_magazine_proofs(magazine_id,input_sha256,verification) VALUES(?1,?2,?3)",params![id.0,hash,proof.to_string()]).unwrap();
    assert!(finalize(&mut c, &id).is_err());
    assert!(mark_reviewed(&c, &id, "wrong").is_err());
    mark_reviewed(&c, &id, &sha).unwrap();
    finalize(&mut c, &id).unwrap();
    assert!(load_issue(&c, &id).unwrap().finalized_at.is_some());
    assert!(library::body(&c, &a).unwrap().article.read_at.is_none());
    assert!(
        c.execute(
            "UPDATE magazine_articles SET html='changed' WHERE magazine_id=?1",
            [&id.0]
        )
        .is_err()
    );
    assert!(delete_draft(&mut c, &id).is_err());
    assert!(
        c.execute("DELETE FROM magazines WHERE id=?1", [&id.0])
            .is_err()
    );
    assert!(
        c.execute(
            "UPDATE native_magazine_proofs SET verification='{}' WHERE magazine_id=?1",
            [&id.0]
        )
        .is_err()
    );
}
#[test]
fn versions_and_bounds_are_explicit() {
    for version in [
        "japanese-watercolor-v1",
        "japanese-watercolor-v2",
        "japanese-watercolor-v3",
        "japanese-watercolor-v4",
        "japanese-watercolor-v5",
        "japanese-watercolor-back-v1",
        "japanese-watercolor-back-v2",
    ] {
        assert!(!prompt(version).unwrap().is_empty());
    }
    assert!(prompt("unknown").is_err());
    let mut c = db();
    let a = article(&mut c, 1, "a.example", 61_000);
    assert!(save_draft(&mut c, &input(vec![a])).is_err());
    let theme = Design {
        profile: PrintProfile::Canva,
        columns: Columns::One,
        ..Design::default()
    };
    let b = article(&mut c, 2, "b.example", 1000);
    let mut draft = input(vec![b]);
    draft.design = theme;
    assert!(save_draft(&mut c, &draft).is_err());
}

#[test]
fn repair_request_captures_immutable_placement_without_changing_legacy_asset_provenance() {
    let mut c = db();
    let article_id = article(&mut c, 1, "repair.example", 1000);
    let magazine_id = save_draft(&mut c, &input(vec![article_id.clone()])).unwrap();
    let context = RepairContext {
        request_id: uuid::Uuid::new_v4(),
        edit_generation: uuid::Uuid::new_v4(),
        magazine_id: magazine_id.clone(),
        article_id,
        after_text: "Unique original paragraph 1.".into(),
        alt: "Original chart".into(),
        note: "Captured source note".into(),
    };
    let mut repair = RepairInput {
        context: Some(Box::new(context.clone())),
        magazine_id: magazine_id.clone(),
        body: png(45),
        label: "Original chart".into(),
        source_url: "https://repair.example/1".into(),
        image_url: "https://repair.example/chart.png".into(),
        retrieved_at: "2026-10-22T00:00:00Z".into(),
    };
    repair.context.as_mut().unwrap().magazine_id =
        macos_read_later::domain::MagazineId("wrong-draft".into());
    assert!(add_repair(&mut c, &repair).is_err());
    repair.context = Some(Box::new(context.clone()));
    repair.context.as_mut().unwrap().article_id = ArticleId("wrong-article".into());
    assert!(add_repair(&mut c, &repair).is_err());
    let originals: i64 = c
        .query_row(
            "SELECT count(*) FROM native_magazine_source_repairs",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(originals, 0);
    repair.context = Some(Box::new(context.clone()));
    let Event::Repair(result) = execute(&mut c, Command::AddRepair(repair)).unwrap() else {
        panic!("expected repair result");
    };
    assert_eq!(result.context, Some(Box::new(context)));
    let provenance: String = c
        .query_row(
            "SELECT provenance FROM native_magazine_source_repairs WHERE asset_id=?1",
            [&result.asset.id],
            |row| row.get(0),
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&provenance).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 5);
    assert!(json.get("context").is_none());
    assert!(json.get("edit_generation").is_none());
    let legacy: RepairAsset = serde_json::from_value(json).unwrap();
    assert_eq!(legacy.sha256, result.asset.sha256);
}
