use anyhow::{Result, bail};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSSize};
use reading_list_rendering_proof::{
    native_pdf::{PdfWindow, inspect_pdf},
    native_text::NativeArticleWindow,
    rich_content::{RichDocument, native_rtf},
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf};
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn main() -> Result<()> {
    let mtm =
        MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("Proof must run on main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let output = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".local/rendering-proof"));
    if args.first().is_some_and(|a| a == "--render-job") {
        reading_list_rendering_proof::production_renderer::render_job(
            &PathBuf::from(&args[1]),
            &PathBuf::from(&args[2]),
        )
        .map_err(anyhow::Error::msg)?;
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--inspect-pdf") {
        let bytes = std::fs::read(&args[1])?;
        let (pages, text) = inspect_pdf(&bytes).map_err(anyhow::Error::msg)?;
        std::fs::write(&args[2], &text)?;
        println!("Native PDFKit inspected {pages} pages");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--raster-pdf") {
        use objc2::AnyThread;
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
        use objc2_foundation::{NSData, NSDictionary};
        use objc2_pdf_kit::{PDFDisplayBox, PDFDocument};
        let bytes = std::fs::read(&args[1])?;
        let pdf =
            unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &NSData::with_bytes(&bytes)) }
                .ok_or_else(|| anyhow::anyhow!("Invalid PDF"))?;
        let count = unsafe { pdf.pageCount() };
        let dir = PathBuf::from(&args[2]);
        std::fs::create_dir_all(&dir)?;
        for index in [0, 1, count / 2, count - 2, count - 1]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            let page = unsafe { pdf.pageAtIndex(index) }
                .ok_or_else(|| anyhow::anyhow!("Missing PDF page"))?;
            let image = unsafe {
                page.thumbnailOfSize_forBox(NSSize::new(918.0, 1188.0), PDFDisplayBox::MediaBox)
            };
            let data = image
                .TIFFRepresentation()
                .ok_or_else(|| anyhow::anyhow!("PDF raster failed"))?;
            let bitmap = NSBitmapImageRep::imageRepWithData(&data)
                .ok_or_else(|| anyhow::anyhow!("Native bitmap failed"))?;
            let png = unsafe {
                bitmap.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }
            .ok_or_else(|| anyhow::anyhow!("Native PNG failed"))?;
            std::fs::write(dir.join(format!("page-{:03}.png", index + 1)), png.to_vec())?;
        }
        println!("Native PDFKit rasterized representative pages from {count}-page PDF");
        return Ok(());
    }
    std::fs::create_dir_all(&output)?;
    if args.iter().any(|a| a == "--retained") {
        return retained(&output);
    }

    let mut html = String::from(
        "<h1>Native reading proof</h1><p>Selectable <strong>bold</strong>, <em>italic</em>, <a href='https://example.org/source'>source link</a>, superscript<sup>1</sup> and H<sub>2</sub>O.</p><table><caption>Measured results</caption><tr><th colspan='2'>Two columns</th></tr><tr><td>α cell</td><td>β cell</td></tr></table><figure><img src='/api/media/synthetic' alt='Saved raster chart'><figcaption>Cached chart caption.</figcaption></figure><pre>fn main() {\n  println!(\"native\");\n}</pre><ul><li>First item</li><li>Second item</li></ul><p>Footnote<sup><a href='#note1'>1</a></sup>: Original evidence is preserved.</p>",
    );
    for index in 0..150 {
        html.push_str(&format!("<p>Paragraph {index}. Every retained word must survive native pagination, including Unicode café, αβγ, and the deliberate ending sentinel. The reader stores complete source evidence, safely renders rich structures, and never fetches publisher images while displaying a saved article.</p>"));
    }
    html.push_str("<p id='note1'>FINAL ENDING SENTINEL 8675309.</p>");
    let doc = RichDocument::parse(&html);
    let image = image::RgbaImage::from_fn(200, 100, |x, y| {
        image::Rgba([(x % 255) as u8, (y % 255) as u8, 70, 255])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut bytes, image::ImageFormat::Png)?;
    let mut images = HashMap::new();
    images.insert("/api/media/synthetic".into(), bytes.into_inner());
    let rtf = native_rtf(&doc, &images, 19.0, true).map_err(anyhow::Error::msg)?;
    std::fs::write(output.join("proof.rtf"), &rtf)?;
    let native = reading_list_rendering_proof::rich_content::native::attributed_document(
        &doc, &images, 19.0, true, 780.0,
    )
    .map_err(anyhow::Error::msg)?;
    let native_text = native.string().to_string();
    let native_attachment_count = native_text.chars().filter(|c| *c == '\u{fffc}').count();
    if native_attachment_count != 1 {
        bail!("Native RTF decoder lost cached raster; attachments {native_attachment_count}");
    }
    for expected in [
        "Two columns",
        "α cell",
        "β cell",
        "FINAL ENDING SENTINEL 8675309",
        "native",
    ] {
        if !native_text.contains(expected) {
            bail!("Native rich decoder lost {expected}");
        }
    }
    let viewer = NativeArticleWindow::open_attributed(&native).map_err(anyhow::Error::msg)?;
    let manager = unsafe { viewer.text.layoutManager() }
        .ok_or_else(|| anyhow::anyhow!("Native layout missing"))?;
    let container = unsafe { viewer.text.textContainer() }
        .ok_or_else(|| anyhow::anyhow!("Native text container missing"))?;
    manager.ensureLayoutForTextContainer(&container);
    let bounds = |needle: &str| {
        let range = native
            .string()
            .rangeOfString(&objc2_foundation::NSString::from_str(needle));
        let glyphs = unsafe {
            manager.glyphRangeForCharacterRange_actualCharacterRange(range, std::ptr::null_mut())
        };
        manager.boundingRectForGlyphRange_inTextContainer(glyphs, &container)
    };
    let left = bounds("α cell");
    let right = bounds("β cell");
    if (left.origin.y - right.origin.y).abs() > 1.0 || right.origin.x <= left.origin.x {
        bail!("Native two-column table cells lost their row geometry: {left:?} / {right:?}");
    }
    let _delegate = reading_list_rendering_proof::rich_content::native::install_reader_delegate(
        &viewer.text,
        &doc,
    )
    .map_err(anyhow::Error::msg)?;
    let range = viewer
        .text
        .string()
        .rangeOfString(&objc2_foundation::NSString::from_str(
            "FINAL ENDING SENTINEL",
        ));
    unsafe {
        viewer
            .text
            .clickedOnLink_atIndex(&objc2_foundation::NSString::from_str("#note1"), 0);
    }
    if viewer.text.selectedRange().location != range.location {
        bail!("Native footnote click did not navigate to saved anchor");
    }
    let source_range = native
        .string()
        .rangeOfString(&objc2_foundation::NSString::from_str("source link"));
    let source_link = unsafe {
        native.attribute_atIndex_effectiveRange(
            objc2_app_kit::NSLinkAttributeName,
            source_range.location,
            std::ptr::null_mut(),
        )
    };
    if source_link.is_none() {
        bail!("Native rich decoder lost external hyperlink");
    }
    let pdf = viewer
        .pdf(NSSize::new(612.0, 792.0))
        .map_err(anyhow::Error::msg)?;
    std::fs::write(output.join("proof.pdf"), &pdf)?;
    let (page_count, pdf_text) = inspect_pdf(&pdf).map_err(anyhow::Error::msg)?;
    let actual = normalized(&pdf_text);
    let expected = normalized(&doc.plain_text());
    if !actual.contains(&normalized("FINAL ENDING SENTINEL 8675309")) {
        bail!("PDF omitted prose ending; {page_count} pages");
    }
    std::fs::write(output.join("actual.txt"), &actual)?;
    std::fs::write(output.join("expected.txt"), &expected)?;
    let complete = actual.contains(&expected);
    if args.iter().any(|a| a == "--host-proof") {
        use objc2_app_kit::{NSButton, NSScrollView};
        use reading_list_rendering_proof::{
            domain::ArticleId,
            rich_content::native::{NativeRichView, ReaderActionKind, ReaderHeader},
        };
        let original = viewer
            .window
            .contentView()
            .ok_or_else(|| anyhow::anyhow!("Missing proof window view"))?;
        let pointer =
            std::ptr::NonNull::new(objc2::rc::Retained::as_ptr(&original) as *mut std::ffi::c_void)
                .unwrap();
        let mut host = unsafe { NativeRichView::attach(pointer) }.map_err(anyhow::Error::msg)?;
        host.set_header(
            ReaderHeader {
                article_id: ArticleId("native-host-proof".into()),
                publication: "Saved source".into(),
                title: "Whole native reader header".into(),
                metadata: "Author · 8 Oct 2026".into(),
                read: false,
                archived: false,
                notice: None,
            },
            reading_list_rendering_proof::reader_style::ReaderFont::Georgia,
        )
        .map_err(anyhow::Error::msg)?;
        host.set_document(
            &doc,
            &images,
            19.0,
            reading_list_rendering_proof::reader_style::ReaderFont::Georgia,
        )
        .map_err(anyhow::Error::msg)?;
        let size = original.bounds().size;
        let viewport = egui::vec2(size.width as f32, size.height as f32);
        for width in [size.width as f32, 420.0] {
            host.set_rect(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, viewport.y)),
                viewport,
            );
        }
        host.set_visible(true);
        let wrapper = viewer.window.contentView().unwrap();
        let scroll = wrapper
            .subviews()
            .iter()
            .filter_map(|view| view.clone().downcast::<NSScrollView>().ok())
            .last()
            .ok_or_else(|| anyhow::anyhow!("Native reader is not a sibling of host controls"))?;
        let document = scroll.documentView().unwrap();
        let button = document
            .subviews()
            .iter()
            .find_map(|view| {
                view.clone()
                    .downcast::<NSButton>()
                    .ok()
                    .filter(|button| button.tag() == 0)
            })
            .ok_or_else(|| anyhow::anyhow!("Native read action missing"))?;
        unsafe { button.performClick(None) };
        let actions = host.take_actions();
        if actions.len() != 1 || !matches!(actions[0].kind, ReaderActionKind::ToggleRead(true)) {
            bail!("Native button did not emit the typed article action");
        }
        host.select_all();
        if host.selected_text() != native_text {
            bail!("Native host body selection lost text");
        }
        host.set_visible(false);
        if viewer.window.firstResponder().is_none() {
            bail!("Native host failed to restore keyboard focus");
        }
    }
    let report = serde_json::json!({"source_sha256":doc.source_sha256,"rtf_sha256":format!("{:x}",Sha256::digest(&rtf)),"pdf_sha256":format!("{:x}",Sha256::digest(&pdf)),"page_count":page_count,"native_attachment_count":native_attachment_count,"native_table_two_column_geometry":true,"native_reader_host":args.iter().any(|a|a=="--host-proof"),"footnote_native_click":true,"external_link_attribute":true,"expected_text_characters":expected.len(),"pdf_text_characters":actual.len(),"complete_text_in_order":complete,"ending_present":true,"native_decoder_text_sha256":format!("{:x}",Sha256::digest(native_text)),"runtime":"Rust + AppKit/PDFKit only"});
    std::fs::write(
        output.join("verification.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if args.iter().any(|a| a == "--show") {
        let _pdf_viewer =
            PdfWindow::open(&pdf, "Exact native production PDF").map_err(anyhow::Error::msg)?;
        app.finishLaunching();
        app.activate();
        app.run();
    }
    if !complete {
        bail!("Full PDF text order verification failed; inspect private report and PDF");
    }
    Ok(())
}

fn retained(output: &std::path::Path) -> Result<()> {
    let records: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(output.join("retained/manifest.json"))?)?;
    let mut reports = Vec::new();
    for record in records {
        let kind = record["kind"].as_str().unwrap();
        if let Some(file) = record["html_file"].as_str() {
            let html = std::fs::read_to_string(file)?;
            let doc = RichDocument::parse(&html);
            let mut images = HashMap::new();
            for media in record["media"].as_array().unwrap() {
                images.insert(
                    format!("/api/media/{}", media["id"].as_str().unwrap()),
                    std::fs::read(media["file"].as_str().unwrap())?,
                );
            }
            let attributed =
                reading_list_rendering_proof::rich_content::native::attributed_document(
                    &doc, &images, 19.0, true, 520.0,
                )
                .map_err(anyhow::Error::msg)?;
            let actual = attributed.string().to_string();
            let attachments = actual.chars().filter(|c| *c == '\u{fffc}').count();
            if attachments != doc.image_sources().len() {
                bail!("Retained image count differs: {kind}");
            }
            let viewer =
                NativeArticleWindow::open_attributed(&attributed).map_err(anyhow::Error::msg)?;
            viewer
                .text
                .setSelectedRange(objc2_foundation::NSRange::new(0, attributed.length()));
            let selected = viewer
                .text
                .string()
                .substringWithRange(viewer.text.selectedRange())
                .to_string();
            if selected != actual {
                bail!("Native continuous selection lost content");
            }
            let pdf = viewer
                .pdf(NSSize::new(612.0, 792.0))
                .map_err(anyhow::Error::msg)?;
            let (pages, text) = inspect_pdf(&pdf).map_err(anyhow::Error::msg)?;
            let complete =
                reading_list_rendering_proof::rich_content::verify_rendered_text(&doc, &text)
                    .is_ok();
            std::fs::write(output.join(format!("retained-{kind}.pdf")), &pdf)?;
            reports.push(serde_json::json!({"kind":kind,"source_sha256":doc.source_sha256,"complete_text":complete,"images":attachments,"selection_complete":true,"pages":pages,"pdf_sha256":format!("{:x}",Sha256::digest(&pdf))}));
            if !complete {
                std::fs::write(
                    output.join(format!("retained-{kind}-expected.txt")),
                    normalized(&doc.plain_text()),
                )?;
                std::fs::write(
                    output.join(format!("retained-{kind}-actual.txt")),
                    normalized(&text),
                )?;
                bail!("Retained {kind} PDF lost or duplicated prose");
            }
        } else {
            let bytes = std::fs::read(record["file"].as_str().unwrap())?;
            let viewer = PdfWindow::open(&bytes, "Read Later exact stored PDF verification")
                .map_err(anyhow::Error::msg)?;
            if viewer.page_count() != record["pages"].as_u64().unwrap() as usize
                || viewer.original_sha256 != record["sha256"].as_str().unwrap()
            {
                bail!("Stored exact PDF changed");
            }
            reports.push(serde_json::json!({"kind":kind,"id":record["id"],"pages":viewer.page_count(),"sha256":viewer.original_sha256,"native_pdfkit_loaded":true}));
        }
    }
    std::fs::write(
        output.join("retained-verification.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    println!("Retained article/PDF verification complete");
    Ok(())
}
