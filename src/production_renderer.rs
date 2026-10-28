//! Offline native magazine renderer, called by this app's isolated Rust child process.
use crate::magazine::RenderJob;
use std::path::Path;

pub fn render_job(input: &Path, output: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(input).map_err(|e| e.to_string())?;
    if metadata.len() > 300_000_000 {
        return Err("Magazine render input exceeds private job limit".into());
    }
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let job: RenderJob =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid render job: {e}"))?;
    #[cfg(target_os = "macos")]
    {
        let output = if output.is_absolute() {
            output.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(output)
        };
        macos::render(&job, &output)?;
        use std::os::unix::fs::PermissionsExt;
        for path in [&output, &output.with_extension("verification.json")] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (job, output);
        Err("Native magazine production requires macOS".into())
    }
}
/// Count every intermediate file against the renderer's allocated workspace.
fn write_bounded(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let parent = path.parent().ok_or("Renderer output has no directory")?;
    let budget = parent.join("storage-budget.json");
    if budget.exists() {
        let policy: serde_json::Value =
            serde_json::from_slice(&std::fs::read(budget).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let limit = policy["pdf_limit"]
            .as_u64()
            .ok_or("Missing PDF reservation")?;
        if path.extension().is_some_and(|extension| extension == "pdf")
            && bytes.len() as u64 > limit
        {
            return Err(
                "Complete edition exceeds its reserved PDF space; no content was truncated".into(),
            );
        }
        let existing = std::fs::metadata(path)
            .map(|metadata| metadata.len().max(metadata.blocks().saturating_mul(512)))
            .unwrap_or(0);
        let size = crate::storage::measured_bytes(parent).map_err(|e| e.to_string())?;
        // Reserve whole filesystem blocks before writing, including the final
        // verification receipt. The parent accounts for all other intermediates.
        let incoming = (bytes.len() as u64).div_ceil(4096).saturating_mul(4096);
        let total = size.saturating_sub(existing).saturating_add(incoming);
        if total
            > policy["workspace_limit"]
                .as_u64()
                .ok_or("Missing workspace reservation")?
        {
            return Err("Complete edition exceeds its reserved working space".into());
        }
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod budget_tests {
    use super::write_bounded;
    fn policy(directory: &std::path::Path, pdf_limit: u64, workspace_limit: u64) {
        std::fs::write(
            directory.join("storage-budget.json"),
            serde_json::to_vec(&serde_json::json!({
                "pdf_limit": pdf_limit,
                "workspace_limit": workspace_limit
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn refuses_pdf_before_creating_an_oversized_file() {
        let directory = tempfile::tempdir().unwrap();
        policy(directory.path(), 16, 100_000);
        let output = directory.path().join("proof.pdf");
        let error = write_bounded(&output, &[0; 17]).unwrap_err();
        assert!(error.contains("reserved PDF"));
        assert!(!output.exists());
    }

    #[test]
    fn verification_receipt_shares_the_workspace_budget() {
        let directory = tempfile::tempdir().unwrap();
        policy(directory.path(), 16_384, 16_384);
        write_bounded(&directory.path().join("proof.pdf"), &[0; 8192]).unwrap();
        let receipt = directory.path().join("proof.verification.json");
        let error = write_bounded(&receipt, &[0; 8192]).unwrap_err();
        assert!(error.contains("reserved working space"));
        assert!(!receipt.exists());
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::rich_content::{
        RichDocument,
        native::{attributed_from_rtf, attributed_from_rtf_with_image_limits},
        native_rtf,
    };
    use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained};
    use objc2_app_kit::{
        NSApplication, NSColor, NSCompositingOperation, NSImage, NSLayoutManager, NSPrintInfo,
        NSPrintOperation, NSPrintingPaginationMode, NSTextContainer, NSTextStorage, NSView,
    };
    use objc2_foundation::{
        MainThreadMarker, NSAttributedString, NSCopying, NSMutableData, NSPoint, NSRange, NSRect,
        NSSize, NSString, NSURL,
    };
    use objc2_pdf_kit::{
        PDFActionGoTo, PDFActionURL, PDFAnnotation, PDFAnnotationSubtypeLink, PDFDestination,
        PDFDisplayBox, PDFDocument,
    };
    use sha2::{Digest, Sha256};
    use std::time::Instant;
    use std::{cell::Cell, collections::HashMap};

    struct PrintIvars {
        _storage: Retained<NSTextStorage>,
        manager: Retained<NSLayoutManager>,
        containers: Vec<Retained<NSTextContainer>>,
        columns: usize,
        column_width: f64,
        gap: f64,
        page_height: f64,
        page_count: usize,
        selected_page: Cell<usize>,
        margin: f64,
        paper: Retained<NSColor>,
    }
    struct PrintGeometry<'a> {
        width: f64,
        height: f64,
        columns: usize,
        gap: f64,
        paper_color: &'a str,
        margin: f64,
    }
    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind=MainThreadOnly]
        #[ivars=PrintIvars]
        #[name="ReadingListMagazinePrintView"]
        struct MagazinePrintView;
    impl MagazinePrintView {
            #[unsafe(method(locationOfPrintRect:))]
            fn location_of_print_rect(&self,_rect:NSRect)->NSPoint{NSPoint::new(0.0,0.0)}
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self)->bool {true}
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self,_dirty:NSRect) {
                let v=self.ivars();let page=v.selected_page.get();
                v.paper.setFill();objc2_app_kit::NSRectFill(NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(v.column_width*v.columns as f64+v.gap*(v.columns-1)as f64+v.margin*2.0,v.page_height)));
                for column in 0..v.columns {if let Some(container)=v.containers.get(page*v.columns+column){let range=v.manager.glyphRangeForTextContainer(container);let origin=NSPoint::new(v.margin+column as f64*(v.column_width+v.gap),v.margin);v.manager.drawBackgroundForGlyphRange_atPoint(range,origin);v.manager.drawGlyphsForGlyphRange_atPoint(range,origin);}}
            }
        }
    );
    impl MagazinePrintView {
        fn create(
            attributed: &NSAttributedString,
            geometry: PrintGeometry<'_>,
            deadline: Instant,
            mtm: MainThreadMarker,
        ) -> Result<Retained<Self>, String> {
            let PrintGeometry {
                width,
                height,
                columns,
                gap,
                paper_color,
                margin,
            } = geometry;
            let storage = NSTextStorage::new();
            storage.setAttributedString(attributed);
            let manager = NSLayoutManager::new();
            storage.addLayoutManager(&manager);
            let column_width = (width - margin * 2.0 - gap * (columns - 1) as f64) / columns as f64;
            let total = manager.numberOfGlyphs();
            let mut covered = 0;
            let mut containers = Vec::new();
            while covered < total {
                if deadline.elapsed().as_secs() > 300 {
                    return Err("Magazine production exceeded five minutes".into());
                }
                if containers.len() >= 400 * columns {
                    return Err("Magazine exceeds 400 complete pages".into());
                }
                let container = NSTextContainer::initWithContainerSize(
                    NSTextContainer::alloc(),
                    NSSize::new(column_width, height - margin * 2.0),
                );
                container.setLineFragmentPadding(0.0);
                manager.addTextContainer(&container);
                manager.ensureLayoutForTextContainer(&container);
                let range = manager.glyphRangeForTextContainer(&container);
                if range.length == 0 {
                    return Err(
                        "A saved structure cannot fit the selected paper and columns".into(),
                    );
                }
                covered = range.location + range.length;
                containers.push(container);
            }
            let page_count = containers.len().div_ceil(columns).max(1);
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
            let rgb = u32::from_str_radix(paper_color.trim_start_matches('#'), 16)
                .map_err(|_| "Invalid paper color")?;
            let paper = NSColor::colorWithSRGBRed_green_blue_alpha(
                ((rgb >> 16) & 255) as f64 / 255.0,
                ((rgb >> 8) & 255) as f64 / 255.0,
                (rgb & 255) as f64 / 255.0,
                1.0,
            );
            let this = Self::alloc(mtm).set_ivars(PrintIvars {
                _storage: storage,
                manager,
                containers,
                columns,
                column_width,
                gap,
                page_height: height,
                page_count,
                selected_page: Cell::new(0),
                margin,
                paper,
            });
            Ok(unsafe { msg_send![super(this),initWithFrame:frame] })
        }
    }
    struct CoverIvars {
        _storage: Retained<NSTextStorage>,
        manager: Retained<NSLayoutManager>,
        container: Retained<NSTextContainer>,
        image: Option<Retained<NSImage>>,
        paper: Retained<NSColor>,
        size: NSSize,
        margin: f64,
    }
    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind=MainThreadOnly]
        #[ivars=CoverIvars]
        #[name="ReadingListMagazineCoverView"]
        struct CoverView;
        impl CoverView {
            #[unsafe(method(knowsPageRange:))] fn knows_page_range(&self,range:*mut NSRange)->bool{if range.is_null(){false}else{unsafe{*range=NSRange::new(1,1);}true}}
            #[unsafe(method(rectForPage:))] fn rect_for_page(&self,_page:isize)->NSRect{NSRect::new(NSPoint::new(0.0,0.0),self.ivars().size)}
            #[unsafe(method(locationOfPrintRect:))]
            fn location_of_print_rect(&self,_rect:NSRect)->NSPoint{NSPoint::new(0.0,0.0)}
            #[unsafe(method(isFlipped))] fn is_flipped(&self)->bool{true}
            #[unsafe(method(drawRect:))] fn draw_rect(&self,_dirty:NSRect){
                let v=self.ivars();v.paper.setFill();let bounds=NSRect::new(NSPoint::new(0.0,0.0),v.size);objc2_app_kit::NSRectFill(bounds);
                if let Some(image)=&v.image{let size=image.size();let scale=(v.size.width/size.width).max(v.size.height/size.height);let source=NSRect::new(NSPoint::new((size.width-v.size.width/scale)/2.0,(size.height-v.size.height/scale)/2.0),NSSize::new(v.size.width/scale,v.size.height/scale));unsafe{image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(bounds,source,NSCompositingOperation::SourceOver,1.0,true,None);}}
                let range=v.manager.glyphRangeForTextContainer(&v.container);v.manager.drawGlyphsForGlyphRange_atPoint(range,NSPoint::new(v.margin,v.margin));
            }
        }
    );
    fn cover_pdf(job: &RenderJob, front: bool, output: &Path) -> Result<(), String> {
        let issue = &job.issue;
        let d = &issue.design;
        let (width, height) = issue.paper_size.points();
        let mtm = MainThreadMarker::new().ok_or("Cover renderer requires main thread")?;
        let asset = if front {
            d.front_asset.as_ref()
        } else {
            d.back_asset.as_ref()
        };
        let image = asset
            .map(|id| {
                let bytes = job
                    .media
                    .get(&format!("/api/magazine-artwork/{id}"))
                    .ok_or("Cover print derivative is missing")?;
                NSImage::initWithData(
                    NSImage::alloc(),
                    &objc2_foundation::NSData::with_bytes(bytes),
                )
                .ok_or("Cover print derivative is corrupt")
            })
            .transpose()?;
        let color = u32::from_str_radix(d.theme.paper.trim_start_matches('#'), 16)
            .map_err(|_| "Invalid cover paper color")?;
        let paper = NSColor::colorWithSRGBRed_green_blue_alpha(
            ((color >> 16) & 255) as f64 / 255.0,
            ((color >> 8) & 255) as f64 / 255.0,
            (color & 255) as f64 / 255.0,
            1.0,
        );
        let mut html = String::new();
        if front {
            html.push_str(&format!(
                "<h1>{}</h1><p>{}</p><p>Issue {}</p>",
                html_escape(&issue.title),
                html_escape(&issue.subtitle),
                issue.issue_number
            ));
            if d.cover_essay_titles {
                for article in &issue.articles {
                    html.push_str(&format!("<p>{}</p>", html_escape(&article.title)));
                }
            }
        }
        let doc = RichDocument::parse(&html);
        let mut rtf = String::from_utf8(native_rtf(&doc, &HashMap::new(), 9.0, true)?)
            .map_err(|e| e.to_string())?;
        rtf = rtf
            .replace("\\b\\fs64", "\\b\\fs40")
            .replace("\\sl444", "\\sl280")
            .replace(
                "\\f0 Georgia;",
                &format!("\\f0 {};", d.theme.display_font.as_str()),
            );
        let attributed = attributed_from_rtf(&doc, &HashMap::new(), rtf.as_bytes(), width, height)?;
        let storage = NSTextStorage::new();
        storage.setAttributedString(&attributed);
        let manager = NSLayoutManager::new();
        storage.addLayoutManager(&manager);
        let margin = issue.paper_size.margin();
        let container = NSTextContainer::initWithContainerSize(
            NSTextContainer::alloc(),
            NSSize::new(width * 0.48 - margin, height * 0.45 - margin),
        );
        container.setLineFragmentPadding(0.0);
        manager.addTextContainer(&container);
        manager.ensureLayoutForTextContainer(&container);
        if manager.glyphRangeForTextContainer(&container).length < manager.numberOfGlyphs() {
            return Err("Cover copy overflows the reserved ivory top-left area; shorten the title/subtitle or split the edition".into());
        }
        let this = CoverView::alloc(mtm).set_ivars(CoverIvars {
            _storage: storage,
            manager,
            container,
            image,
            paper,
            size: NSSize::new(width, height),
            margin,
        });
        let view: Retained<CoverView> = unsafe {
            msg_send![super(this),initWithFrame:NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(width,height))]
        };
        save_view(&view, NSSize::new(width, height), 0.0, output)
    }
    /// Verify the text actually drawn at each native line's PDF coordinates.
    /// PDFKit's whole-page extraction reorders table cells and column-boundary
    /// footers, so layout ranges determine reading order rather than that heuristic.
    fn verify_printed_text(
        pdf: &PDFDocument,
        attributed: &NSAttributedString,
        view: &MagazinePrintView,
        interior_text: &str,
    ) -> Result<usize, String> {
        let expected = attributed.string();
        let inventory = |text: &str| {
            let mut counts = std::collections::BTreeMap::new();
            for c in normalized(text).chars() {
                *counts.entry(c).or_insert(0usize) += 1;
            }
            counts
        };
        if inventory(&expected.to_string()) != inventory(interior_text) {
            return Err("Printed text inventory differs from the complete native snapshot".into());
        }
        let v = view.ivars();
        let mut checks = 0;
        for (index, container) in v.containers.iter().enumerate() {
            let owned = v.manager.glyphRangeForTextContainer(container);
            let end = owned.location + owned.length;
            let mut glyph = owned.location;
            let page = unsafe { pdf.pageAtIndex(index / v.columns + 1) }
                .ok_or("Printed text page missing")?;
            while glyph < end {
                let mut line = NSRange::new(glyph, 1);
                unsafe {
                    v.manager
                        .lineFragmentRectForGlyphAtIndex_effectiveRange(glyph, &mut line);
                }
                let start = glyph.max(line.location);
                let stop = end.min(line.location + line.length);
                if stop <= start {
                    return Err("Native line verification made no progress".into());
                }
                let range = NSRange::new(start, stop - start);
                let characters = unsafe {
                    v.manager
                        .characterRangeForGlyphRange_actualGlyphRange(range, std::ptr::null_mut())
                };
                let phrase = normalized(&expected.substringWithRange(characters).to_string());
                if !phrase.is_empty() {
                    let bounds = v
                        .manager
                        .boundingRectForGlyphRange_inTextContainer(range, container);
                    let rect = NSRect::new(
                        NSPoint::new(
                            v.margin
                                + (index % v.columns) as f64 * (v.column_width + v.gap)
                                + bounds.origin.x
                                - 0.5,
                            v.page_height - v.margin - bounds.origin.y - bounds.size.height - 0.5,
                        ),
                        NSSize::new(bounds.size.width + 1.0, bounds.size.height + 1.0),
                    );
                    let selected = unsafe { page.selectionForRect(rect) }
                        .and_then(|selection| unsafe { selection.string() })
                        .map(|text| normalized(&text.to_string()))
                        .unwrap_or_default();
                    if !selected.contains(&phrase) {
                        return Err(format!(
                            "Printed prose geometry mismatch on page {} at glyph {start}",
                            index / v.columns + 2
                        ));
                    }
                    checks += 1;
                }
                glyph = stop;
            }
        }
        Ok(checks)
    }
    fn pdf_links(
        pdf: &PDFDocument,
        document: &RichDocument,
        attributed: &NSAttributedString,
        view: &MagazinePrintView,
        paper_height: f64,
        margin: f64,
    ) -> Result<(usize, usize), String> {
        let layout = view.ivars();
        let anchors = crate::rich_content::native::anchor_ranges(document, &attributed.string());
        let mut count = 0;
        let mut internal_count = 0;
        let mut character = 0;
        while character < attributed.length() {
            let mut range = NSRange::new(character, 1);
            let value = unsafe {
                attributed.attribute_atIndex_effectiveRange(
                    objc2_app_kit::NSLinkAttributeName,
                    character,
                    &mut range,
                )
            };
            character = range.location + range.length.max(1);
            let Some(value) = value else {
                continue;
            };
            let href = if let Some(value) = value.downcast_ref::<NSString>() {
                value.to_string()
            } else if let Some(value) = value.downcast_ref::<NSURL>() {
                value.relativeString().to_string()
            } else {
                return Err("Unsupported native saved link attribute".into());
            };
            let glyphs = unsafe {
                layout
                    .manager
                    .glyphRangeForCharacterRange_actualCharacterRange(range, std::ptr::null_mut())
            };
            for (index, container) in layout.containers.iter().enumerate() {
                let owned = layout.manager.glyphRangeForTextContainer(container);
                let start = glyphs.location.max(owned.location);
                let end = (glyphs.location + glyphs.length).min(owned.location + owned.length);
                if start >= end {
                    continue;
                }
                let bounds = layout.manager.boundingRectForGlyphRange_inTextContainer(
                    NSRange::new(start, end - start),
                    container,
                );
                let page_index = index / layout.columns + 1;
                let page =
                    unsafe { pdf.pageAtIndex(page_index) }.ok_or("Link source page missing")?;
                let x = margin
                    + (index % layout.columns) as f64 * (layout.column_width + layout.gap)
                    + bounds.origin.x;
                let y = paper_height - margin - bounds.origin.y - bounds.size.height;
                let rect = NSRect::new(NSPoint::new(x, y), bounds.size);
                let annotation = unsafe {
                    PDFAnnotation::initWithBounds_forType_withProperties(
                        PDFAnnotation::alloc(),
                        rect,
                        PDFAnnotationSubtypeLink,
                        None,
                    )
                };
                if let Some(anchor) = href.strip_prefix('#') {
                    let target = anchors
                        .get(anchor)
                        .ok_or_else(|| format!("Saved footnote target is missing: {href}"))?;
                    let glyph = layout
                        .manager
                        .glyphIndexForCharacterAtIndex(target.location);
                    let target_index = layout
                        .containers
                        .iter()
                        .position(|container| {
                            let range = layout.manager.glyphRangeForTextContainer(container);
                            glyph >= range.location && glyph < range.location + range.length
                        })
                        .ok_or("Footnote target has no printed page")?;
                    let target_bounds = layout.manager.boundingRectForGlyphRange_inTextContainer(
                        NSRange::new(glyph, 1),
                        &layout.containers[target_index],
                    );
                    let target_page = unsafe { pdf.pageAtIndex(target_index / layout.columns + 1) }
                        .ok_or("Footnote target page missing")?;
                    let point = NSPoint::new(
                        margin
                            + (target_index % layout.columns) as f64
                                * (layout.column_width + layout.gap)
                            + target_bounds.origin.x,
                        paper_height - margin - target_bounds.origin.y,
                    );
                    let destination = unsafe {
                        PDFDestination::initWithPage_atPoint(
                            PDFDestination::alloc(),
                            &target_page,
                            point,
                        )
                    };
                    let action = unsafe {
                        PDFActionGoTo::initWithDestination(PDFActionGoTo::alloc(), &destination)
                    };
                    unsafe {
                        annotation.setAction(Some(&action));
                    }
                    internal_count += 1;
                } else {
                    let parsed = url::Url::parse(&href).map_err(|_| "Invalid saved hyperlink")?;
                    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
                        return Err("Unsafe saved hyperlink scheme".into());
                    }
                    let url = NSURL::URLWithString(&NSString::from_str(&href))
                        .ok_or("Invalid saved hyperlink")?;
                    let action = unsafe { PDFActionURL::initWithURL(PDFActionURL::alloc(), &url) };
                    unsafe {
                        annotation.setAction(Some(&action));
                    }
                }
                unsafe {
                    page.addAnnotation(&annotation);
                }
                count += 1;
            }
        }
        Ok((count, internal_count))
    }
    fn html_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    fn scoped_html(html: &str, scope: &str) -> String {
        let fragment = scraper::Html::parse_fragment(html);
        let selector = scraper::Selector::parse("[id]").expect("static anchor selector");
        let mut output = html.to_owned();
        for element in fragment.select(&selector) {
            if let Some(id) = element.value().attr("id") {
                let scoped = format!("reading-list-{scope}-{id}");
                for quote in ['\"', '\''] {
                    output = output
                        .replace(
                            &format!("id={quote}{id}{quote}"),
                            &format!("id={quote}{scoped}{quote}"),
                        )
                        .replace(
                            &format!("href={quote}#{id}{quote}"),
                            &format!("href={quote}#{scoped}{quote}"),
                        );
                }
            }
        }
        output
    }
    fn normalized(value: &str) -> String {
        value
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    fn color_rtf(hex: &str) -> Result<String, String> {
        let value = hex
            .strip_prefix('#')
            .filter(|s| s.len() == 6)
            .ok_or("Invalid print color")?;
        let rgb = u32::from_str_radix(value, 16).map_err(|_| "Invalid print color")?;
        Ok(format!(
            "\\red{}\\green{}\\blue{}",
            rgb >> 16,
            (rgb >> 8) & 255,
            rgb & 255
        ))
    }
    fn styled_rtf(
        document: &RichDocument,
        job: &RenderJob,
        column_width: f64,
    ) -> Result<Vec<u8>, String> {
        let d = &job.issue.design;
        let rtf = native_rtf(document, &job.media, d.body_size_pt as f32, true)?;
        let mut text = String::from_utf8(rtf).map_err(|e| e.to_string())?;
        text = text
            .replace(
                "\\f0 Georgia;",
                &format!("\\f0 {};", d.theme.body_font.as_str()),
            )
            .replace(
                "\\f2 Helvetica;}",
                &format!(
                    "\\f2 Helvetica;}}{{\\f3 {};}}",
                    d.theme.display_font.as_str()
                ),
            );
        text = text.replace(
            "\\sl444\\slmult1",
            &format!("\\sl{}\\slmult1", (d.line_height * 240.0).round() as u32),
        );
        text = text.replace(
            &format!("\\sa{}", (d.body_size_pt * 2.0).round() as u32 * 5),
            &format!("\\sa{}", (d.paragraph_space_pt * 20.0).round() as u32),
        );
        text = text
            .replace("\\red44\\green48\\blue44", &color_rtf(&d.theme.ink)?)
            .replace("\\red49\\green84\\blue68", &color_rtf(&d.theme.accent)?)
            .replace("\\red133\\green135\\blue127", &color_rtf(&d.theme.muted)?);
        text = text
            .replace("\\b\\fs64", "\\f3\\b\\fs32")
            .replace("\\b\\fs54", "\\f3\\b\\fs30")
            .replace("\\b\\fs46", "\\f3\\b\\fs24");
        let mut scaled = String::new();
        let mut rest = text.as_str();
        while let Some(index) = rest.find("\\cellx") {
            scaled.push_str(&rest[..index + 6]);
            rest = &rest[index + 6..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let old: usize = rest[..digits]
                .parse()
                .map_err(|_| "Invalid native table boundary")?;
            let value = (old as f64 * column_width * 20.0 / 9360.0).round() as usize;
            scaled.push_str(&value.to_string());
            rest = &rest[digits..];
        }
        scaled.push_str(rest);
        Ok(scaled.into_bytes())
    }
    fn view_pdf(view: &NSView, paper: NSSize, margin: f64) -> Result<Vec<u8>, String> {
        let info = NSPrintInfo::sharedPrintInfo().copy();
        info.setPaperSize(paper);
        info.setLeftMargin(margin);
        info.setRightMargin(margin);
        info.setTopMargin(margin);
        info.setBottomMargin(margin);
        info.setHorizontallyCentered(false);
        info.setVerticallyCentered(false);
        info.setHorizontalPagination(NSPrintingPaginationMode::Automatic);
        info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
        // Native printing lays out the full document in memory first. No disk
        // write may exceed the storage reservation before we inspect its bytes.
        let data = NSMutableData::new();
        let operation = NSPrintOperation::PDFOperationWithView_insideRect_toData_printInfo(
            view,
            view.bounds(),
            &data,
            &info,
        );
        operation.setShowsPrintPanel(false);
        operation.setShowsProgressPanel(false);
        if !operation.runOperation() {
            return Err("Native magazine print operation failed".into());
        }
        Ok(data.to_vec())
    }
    fn save_view(view: &NSView, paper: NSSize, margin: f64, path: &Path) -> Result<(), String> {
        write_bounded(path, &view_pdf(view, paper, margin)?)
    }
    fn save_magazine_view(
        view: &MagazinePrintView,
        paper: NSSize,
        path: &Path,
    ) -> Result<(), String> {
        // A PDF copy operation captures one view rectangle. It does not run
        // NSView print pagination, and currentPage is zero. Select each complete
        // layout page explicitly in a one-page view, then combine in memory.
        let combined = unsafe { PDFDocument::new() };
        for index in 0..view.ivars().page_count {
            view.ivars().selected_page.set(index);
            let bytes = view_pdf(view, paper, 0.0)?;
            let page_pdf = unsafe {
                PDFDocument::initWithData(
                    PDFDocument::alloc(),
                    &objc2_foundation::NSData::with_bytes(&bytes),
                )
            }
            .ok_or("Native interior page failed decoding")?;
            if unsafe { page_pdf.pageCount() } != 1 {
                return Err("Native interior copy must contain exactly one page".into());
            }
            let page = unsafe { page_pdf.pageAtIndex(0) }.ok_or("Native interior page missing")?;
            unsafe {
                combined.insertPage_atIndex(&page, index);
            }
        }
        let bytes = unsafe { combined.dataRepresentation() }
            .ok_or("Native interior PDF serialization failed")?
            .to_vec();
        write_bounded(path, &bytes)
    }
    pub fn render(job: &RenderJob, output: &Path) -> Result<(), String> {
        let start = Instant::now();
        let mtm = MainThreadMarker::new()
            .ok_or("Native magazine renderer needs child process main thread")?;
        let _app = NSApplication::sharedApplication(mtm);
        let issue = &job.issue;
        let d = &issue.design;
        if issue.articles.is_empty() || issue.articles.len() > 40 {
            return Err("New editions require 1–40 complete article snapshots".into());
        }
        if !(8.0..=14.0).contains(&d.body_size_pt)
            || !(1.15..=1.8).contains(&d.line_height)
            || !(16.0..=36.0).contains(&d.column_gap_pt)
            || !(3.0..=14.0).contains(&d.paragraph_space_pt)
        {
            return Err("Invalid magazine typography settings".into());
        }
        let (paper_width, paper_height) = issue.paper_size.points();
        let margin = issue.paper_size.margin();
        let width = paper_width - margin * 2.0;
        let height = paper_height - margin * 2.0;
        let columns = d.column_count(issue.paper_size);
        let column_width = (width - d.column_gap_pt * (columns - 1) as f64) / columns as f64;
        let media_bytes: usize = job.media.values().map(Vec::len).sum();
        if media_bytes > 50_000_000 {
            return Err("Edition exceeds 50 MB cached media".into());
        }
        let mut html = String::new();
        let mut original_hashes = Vec::new();
        let mut endings = Vec::new();
        for article in &issue.articles {
            html.push_str(&format!(
                "<h1>{}</h1><p>{}</p>",
                html_escape(&article.title),
                html_escape(
                    article
                        .presentation
                        .display_author
                        .as_deref()
                        .or(article.author.as_deref())
                        .unwrap_or_default()
                )
            ));
            if let Some(source) = &article.presentation.author_source {
                html.push_str(&format!(
                    "<p><a href=\"{}\">Author correction source</a></p>",
                    html_escape(source)
                ));
            }
            if let Some(date) = &article.presentation.published_date {
                html.push_str(&format!(
                    "<p><a href=\"{}\">{}</a></p>",
                    html_escape(&date.source_url),
                    html_escape(&date.display)
                ));
            }
            html.push_str(&scoped_html(&article.presented_html, &article.article_id.0));
            html.push_str(&format!(
                "<p>Source: <a href=\"{}\">{}</a></p>",
                html_escape(&article.url),
                html_escape(&article.url)
            ));
            original_hashes.push(format!("{:x}", Sha256::digest(article.html.as_bytes())));
            endings.push(
                crate::rich_content::RichDocument::parse(&article.presented_html)
                    .plain_text()
                    .trim()
                    .chars()
                    .rev()
                    .take(180)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>(),
            );
        }
        if html.len() > 20_000_000 {
            return Err("Edition exceeds 20 MB snapshot text".into());
        }
        let document = RichDocument::parse(&html);
        let expected_text = normalized(&document.plain_text());
        if expected_text.is_empty() {
            return Err("Edition has no printable content".into());
        }
        let rtf = styled_rtf(&document, job, column_width)?;
        let attributed = attributed_from_rtf_with_image_limits(
            &document,
            &job.media,
            &rtf,
            column_width,
            220.0,
            height * 0.82,
        )?;
        let image_count = document.image_sources().len();
        let native_image_count = attributed
            .string()
            .to_string()
            .chars()
            .filter(|c| *c == '\u{fffc}')
            .count();
        if image_count != native_image_count {
            return Err("Native layout lost cached image attachments".into());
        }
        let view = MagazinePrintView::create(
            &attributed,
            PrintGeometry {
                width: paper_width,
                height: paper_height,
                columns,
                gap: d.column_gap_pt,
                paper_color: &d.theme.paper,
                margin,
            },
            start,
            mtm,
        )?;
        save_magazine_view(&view, NSSize::new(paper_width, paper_height), output)?;
        let bytes = std::fs::read(output).map_err(|e| e.to_string())?;
        if bytes.len() > 52_428_800 {
            return Err("Edition PDF exceeds 50 MB".into());
        }
        let pdf = unsafe {
            PDFDocument::initWithData(
                PDFDocument::alloc(),
                &objc2_foundation::NSData::with_bytes(&bytes),
            )
        }
        .ok_or("Native PDF failed verification decoding")?;
        let front_path = output.with_extension("front.pdf");
        let back_path = output.with_extension("back.pdf");
        cover_pdf(job, true, &front_path)?;
        cover_pdf(job, false, &back_path)?;
        for (path, front) in [(&front_path, true), (&back_path, false)] {
            let cover_bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            let cover = unsafe {
                PDFDocument::initWithData(
                    PDFDocument::alloc(),
                    &objc2_foundation::NSData::with_bytes(&cover_bytes),
                )
            }
            .ok_or("Cover PDF failed decoding")?;
            if unsafe { cover.pageCount() } != 1 {
                return Err("Cover must contain exactly one page".into());
            }
            let page = unsafe { cover.pageAtIndex(0) }.ok_or("Cover page missing")?;
            let index = if front { 0 } else { unsafe { pdf.pageCount() } };
            unsafe {
                pdf.insertPage_atIndex(&page, index);
            }
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        let mut page_count = unsafe { pdf.pageCount() };
        let companion_used = if !page_count.is_multiple_of(2) {
            let asset=d.companion_asset.as_ref().ok_or("Back cover would fall on an odd page; select another whole-article combination or fulfil generated covers to include a front-paint companion")?;
            if d.front_asset.as_ref() != Some(asset) {
                return Err("Parity companion must reuse the approved front painting".into());
            }
            let mut companion = job.clone();
            companion.issue.design.back_asset = Some(asset.clone());
            let path = output.with_extension("companion.pdf");
            cover_pdf(&companion, false, &path)?;
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let companion_pdf = unsafe {
                PDFDocument::initWithData(
                    PDFDocument::alloc(),
                    &objc2_foundation::NSData::with_bytes(&bytes),
                )
            }
            .ok_or("Companion PDF failed decoding")?;
            if unsafe { companion_pdf.pageCount() } != 1 {
                return Err("Parity companion must have exactly one page".into());
            }
            let page =
                unsafe { companion_pdf.pageAtIndex(0) }.ok_or("Parity companion page missing")?;
            unsafe {
                pdf.insertPage_atIndex(&page, page_count - 1);
            }
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            page_count += 1;
            true
        } else {
            false
        };
        if page_count > 400 {
            return Err("Edition exceeds 400 pages including covers".into());
        }
        if d.profile == crate::magazine::PrintProfile::Canva
            && (!(24..=204).contains(&page_count) || !page_count.is_multiple_of(2))
        {
            return Err("Canva requires24–204 even pages; revise whole-article selection without truncation or filler".into());
        }
        let (link_count, internal_link_count) =
            pdf_links(&pdf, &document, &attributed, &view, paper_height, margin)?;
        let bytes = unsafe { pdf.dataRepresentation() }
            .ok_or("Final PDF serialization failed")?
            .to_vec();
        if bytes.len() > 52_428_800 {
            return Err("Final PDF exceeds 50 MB including covers".into());
        }
        write_bounded(output, &bytes)?;
        let actual_text = normalized(
            &unsafe { pdf.string() }
                .ok_or("Native PDF has no selectable text")?
                .to_string(),
        );
        let interior_text = (1..page_count - 1)
            .map(|index| {
                unsafe { pdf.pageAtIndex(index) }
                    .and_then(|page| unsafe { page.string() })
                    .map(|text| text.to_string())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let text_geometry_checks = verify_printed_text(&pdf, &attributed, &view, &interior_text)?;
        for ending in &endings {
            if !normalized(&attributed.string().to_string()).contains(&normalized(ending)) {
                return Err("PDF verification found a missing article ending".into());
            }
        }
        for index in 0..page_count {
            let page = unsafe { pdf.pageAtIndex(index) }.ok_or("PDF page missing")?;
            let bounds = unsafe { page.boundsForBox(PDFDisplayBox::MediaBox) };
            if (bounds.size.width - paper_width).abs() > 0.5
                || (bounds.size.height - paper_height).abs() > 0.5
            {
                return Err("PDF paper geometry verification failed".into());
            }
        }
        let cover_artwork=[("front",d.front_asset.as_ref()),("back",d.back_asset.as_ref())].into_iter().filter_map(|(role,id)|id.map(|id|(role,id))).map(|(role,id)|{let bytes=job.media.get(&format!("/api/magazine-artwork/{id}")).ok_or("Cover derivative disappeared")?;Ok(serde_json::json!({"role":role,"asset_id":id,"print_derivative_sha256":format!("{:x}",Sha256::digest(bytes)),"full_page_geometry":true}))}).collect::<Result<Vec<_>,String>>()?;
        let media_sha256 = job
            .media
            .iter()
            .map(|(source, bytes)| (source.clone(), format!("{:x}", Sha256::digest(bytes))))
            .collect::<std::collections::BTreeMap<_, _>>();
        let report = serde_json::json!({"renderer_version":"reading-list-native-print-2","input_sha256":job.input_sha256,"sha256":format!("{:x}",Sha256::digest(&bytes)),"page_count":page_count,"interior_page_count":view.ivars().page_count,"complete_text":true,"text_geometry_checks":text_geometry_checks,"prose_endings":true,"geometry":true,"cover_copy":actual_text.contains(&normalized(&issue.title))&&actual_text.contains(&normalized(&issue.subtitle)),"article_openings":issue.articles.iter().all(|a|actual_text.contains(&normalized(&a.title))),"links":true,"link_annotations":link_count,"footnote_link_annotations":internal_link_count,"image_count":image_count,"native_image_count":native_image_count,"snapshot_html_sha256":original_hashes,"paper_size":issue.paper_size.as_str(),"columns":columns,"page_parity_even":page_count.is_multiple_of(2),"cover_artwork":cover_artwork,"parity_companion_used":companion_used,"media_sha256":media_sha256,"runtime":"Rust + native AppKit/PDFKit","elapsed_ms":start.elapsed().as_millis()});
        write_bounded(
            &output.with_extension("verification.json"),
            &serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )?;
        Ok(())
    }
}
