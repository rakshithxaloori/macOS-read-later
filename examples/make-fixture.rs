//! Reproducible disposable library for native UI and production checks.
use macos_read_later::{db_compat, domain::*, library, magazine, storage};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{io::Cursor, os::unix::fs::PermissionsExt};
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "render-magazine") {
        let value = |flag: &str| {
            args.windows(2)
                .find(|a| a[0] == flag)
                .map(|a| std::path::PathBuf::from(&a[1]))
                .ok_or_else(|| anyhow::anyhow!("Missing {flag}"))
        };
        return macos_read_later::production_renderer::render_job(
            &value("--input")?,
            &value("--output")?,
        )
        .map_err(anyhow::Error::msg);
    }
    let path = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Supply a new private fixture directory"))?;
    anyhow::ensure!(!path.exists(), "Fixture destination already exists");
    std::fs::create_dir_all(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    let mut c = db_compat::open(&path.join("library.sqlite3"))?;
    let lean = std::env::args().any(|a| a == "--lean");
    if lean {
        db_compat::initialize_lean(&c)?;
    } else {
        db_compat::initialize(&c)?;
    }
    magazine::initialize(&c)?;
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(320, 160, |x, y| {
        image::Rgb([(x % 255) as u8, (y % 255) as u8, 95])
    }))
    .write_to(&mut bytes, image::ImageFormat::Png)?;
    let bytes = bytes.into_inner();
    let mut ids = vec![];
    let mut prepared = vec![];
    for (index, title) in [
        "The architecture of a quiet reading list",
        "Tables, links, images and a last paragraph",
        "An archived article keeps its state",
    ]
    .iter()
    .enumerate()
    {
        let (a, _) = library::save(
            &mut c,
            &format!("https://example.com/native-fixture-{index}"),
            ArrivalKind::Web,
            &format!("fixture-{index}"),
            "Disposable native verification",
            Some(title),
            Some("2026-10-08T00:00:00.000Z"),
        )?;
        let media = uuid::Uuid::new_v4().to_string();
        let mut html = format!(
            "<h2>A saved native reading copy</h2><p>These synthetic articles exercise <strong>bold text</strong>, <em>emphasis</em>, a <a href='https://example.com/reference'>source link</a> and a footnote<a href='#note-1'><sup>1</sup></a>.</p><figure><img src='/api/media/{media}' alt='Synthetic colored pixels'><figcaption>A private raster with exact saved bytes.</figcaption></figure><table><thead><tr><th>Column one</th><th>Column two</th></tr></thead><tbody><tr><td>First retained cell</td><td>Second retained cell</td></tr><tr><td colspan='2'>A complete spanning cell</td></tr></tbody></table><pre><code>let reader = NativeReader::new();</code></pre><ul><li>A whole saved article</li><li>An independently scrolling list</li></ul>"
        );
        if index == 1 {
            for n in 0..240 {
                html.push_str(&format!("<p>Paragraph {n}: durable saved reading copies remain intact across retries and edition production. The native reader preserves links and selection, while background owners publish only under their current leases.</p>"));
            }
        }
        html.push_str("<p id='note-1'>1. A native footnote anchor.</p><p>FINAL NATIVE VERIFICATION SENTINEL: the saved prose ends here.</p>");
        let words = macos_read_later::extraction::word_count(&html);
        c.execute("UPDATE articles SET html=?2,word_count=?3,status='saved',author='Fixture author',site='Example publication',excerpt='Synthetic rich content exercises tables, footnotes and native reading.' WHERE id=?1",params![a.0,if lean{""}else{&html},words])?;
        if lean {
            let s = storage::Storage::for_database(&c)?;
            s.cache_put(
                &a.0,
                s.cache_generation(&a.0)?,
                false,
                storage::CachedArticle {
                    html: html.clone(),
                    media: vec![storage::CachedMedia {
                        id: media.clone(),
                        mime_type: "image/png".into(),
                        bytes: bytes.clone(),
                    }],
                    status: CopyStatus::Saved,
                    error: None,
                    word_count: words,
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                },
            )?;
            if index < 2 {
                prepared.push(magazine::PreparedArticle {
                    snapshot: magazine::Snapshot {
                        article_id: a.clone(),
                        position: index as u32,
                        title: (*title).into(),
                        url: format!("https://example.com/native-fixture-{index}"),
                        author: Some("Fixture author".into()),
                        html: html.clone(),
                        presented_html: html.clone(),
                        word_count: words,
                        content_status: CopyStatus::Saved,
                        content_warning: None,
                        presentation: Default::default(),
                    },
                    media: std::collections::HashMap::from([(
                        format!("/api/media/{media}"),
                        bytes.clone(),
                    )]),
                    source_sha256: format!("{:x}", Sha256::digest(html.as_bytes())),
                    retrieved_at: chrono::Utc::now().to_rfc3339(),
                });
            }
        } else {
            c.execute("INSERT INTO article_media(id,user_id,article_id,source_url,sha256,content_type,body) VALUES(?1,?2,?3,'https://example.com/fixture.png',?4,'image/png',?5)",params![media,OWNER,a.0,format!("{:x}",Sha256::digest(&bytes)),bytes])?;
        }
        if index == 2 {
            library::update(&c, &a, None, Some(true))?;
        } else {
            ids.push(a);
        }
    }
    let input = magazine::DraftInput {
        id: None,
        title: "A native reading proof".into(),
        subtitle: "A disposable edition for complete offline verification".into(),
        paper_size: magazine::PaperSize::Letter,
        article_ids: ids,
        refresh_copies: false,
        design: Default::default(),
        presentation: Default::default(),
    };
    let draft = if lean {
        magazine::save_prepared_draft(&mut c, &input, &prepared)?
    } else {
        magazine::save_draft(&mut c, &input)?
    };
    library::add_feed(&mut c, "https://example.com/fixture-feed.xml")?;
    c.execute("UPDATE feeds SET active=0", [])?;
    if args.iter().any(|a| a == "--artwork") {
        let request = magazine::request_covers(&mut c, &draft)?;
        magazine::fulfil_covers(
            &mut c,
            &magazine::CoverFulfilment {
                magazine_id: draft.clone(),
                request_id: request.id,
                input_sha256: request.input_sha256,
                front: bytes.clone(),
                back: bytes.clone(),
                note: "Synthetic raster generated locally for disposable UI verification. Not publisher or generated painting artwork.".into(),
            },
        )?;
    }
    if args.iter().any(|a| a == "--produce") {
        magazine::queue(&mut c, &draft, false)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let worker = magazine::spawn_worker(
            path.join("library.sqlite3"),
            path.join("production"),
            std::sync::Arc::new(move || {
                let _ = tx.try_send(());
            }),
        );
        let started = std::time::Instant::now();
        let produced = (|| -> anyhow::Result<()> {
            loop {
                let edition = magazine::edition(&c, &draft)?;
                if let Some(p) = edition.production {
                    if p.state == magazine::ProductionState::Done {
                        break;
                    }
                    anyhow::ensure!(
                        p.state != magazine::ProductionState::Failed,
                        "Production failed: {:?}",
                        p.error
                    );
                }
                anyhow::ensure!(
                    started.elapsed().as_secs() < 330,
                    "Production did not complete within its timeout"
                );
                let _ = rx.recv_timeout(std::time::Duration::from_millis(500));
            }
            let pdf = library::stored_pdf(&c, &draft)?;
            let (pages, text) =
                macos_read_later::native_pdf::inspect_pdf(&pdf).map_err(anyhow::Error::msg)?;
            anyhow::ensure!(
                pages > 0 && text.matches("FINAL NATIVE VERIFICATION SENTINEL").count() == 2,
                "Produced PDF lost a complete article ending"
            );
            let sha = format!("{:x}", Sha256::digest(&pdf));
            magazine::mark_reviewed(&c, &draft, &sha)?;
            magazine::finalize(&mut c, &draft)?;
            anyhow::ensure!(
                library::stored_pdf(&c, &draft)? == pdf,
                "Finalization changed exact PDF bytes"
            );
            let remaining:i64=c.query_row("SELECT (SELECT count(*) FROM native_magazine_media)+(SELECT count(*) FROM native_magazine_acquisitions)+(SELECT count(*) FROM magazine_articles WHERE html<>'')",[],|r|r.get(0))?;
            anyhow::ensure!(remaining == 0, "Finalization retained production originals");
            println!(
                "{}",
                serde_json::json!({"verified_native_production":true,"pdf_sha256":sha,"page_count":pages,"pdf_bytes":pdf.len(),"edition_id":draft.0,"managed_bytes":storage::measured_bytes(&path)?})
            );
            Ok(())
        })();
        worker.shutdown();
        produced?;
    }
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    println!("{}", path.display());
    Ok(())
}
