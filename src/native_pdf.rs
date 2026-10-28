//! Exact stored bytes are handed to macOS PDFKit. No regenerated viewer copy.
#[cfg(target_os = "macos")]
mod macos {
    use objc2::{
        AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
    };
    use objc2_app_kit::{
        NSAccessibility, NSAppearance, NSAppearanceCustomization, NSAutoresizingMaskOptions,
        NSBackingStoreType, NSBezelStyle, NSBitmapImageFileType, NSBitmapImageRep, NSBox,
        NSBoxType, NSButton, NSCellImagePosition, NSColor, NSColorRenderingIntent, NSColorSpace,
        NSFont, NSImage, NSLineBreakMode, NSTextAlignment, NSTextField, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{
        MainThreadMarker, NSData, NSDictionary, NSNotification, NSNotificationCenter, NSObject,
        NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use objc2_pdf_kit::{
        PDFDisplayBox, PDFDocument, PDFThumbnailView, PDFView, PDFViewPageChangedNotification,
        PDFViewScaleChangedNotification,
    };
    use sha2::{Digest, Sha256};

    struct ViewerTargetIvars {
        view: Retained<PDFView>,
        document: Retained<PDFDocument>,
        page_label: Retained<NSTextField>,
        title_label: Retained<NSTextField>,
        buttons: Vec<Retained<NSButton>>,
        thumbnails: Retained<PDFThumbnailView>,
    }
    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = ViewerTargetIvars]
        #[name = "ReadingListPdfToolbarTarget"]
        struct ViewerTarget;
        unsafe impl NSObjectProtocol for ViewerTarget {}
        impl ViewerTarget {
            #[unsafe(method(viewerAction:))]
            fn viewer_action(&self, sender: &NSButton) {
                let view = &self.ivars().view;
                unsafe {
                    match sender.tag() {
                        0 => view.goToPreviousPage(None),
                        1 => view.goToNextPage(None),
                        2 => { view.setAutoScales(false); view.zoomOut(None); }
                        3 => { view.setAutoScales(false); view.zoomIn(None); }
                        4 => view.setAutoScales(true),
                        5 => {
                            let thumbnails=&self.ivars().thumbnails;
                            let show=thumbnails.isHidden();
                            thumbnails.setHidden(!show);
                            if let Some(parent)=view.superview(){
                                let frame=view.frame();let x=if show{132.}else{0.};
                                view.setFrame(NSRect::new(NSPoint::new(x,0.),NSSize::new(parent.bounds().size.width-x,frame.size.height)));
                            }
                            view.layoutSubtreeIfNeeded();
                        },
                        _ => return,
                    }
                }
                self.refresh();
            }
            #[unsafe(method(pdfViewChanged:))]
            fn pdf_view_changed(&self, _notification: &NSNotification) {
                self.refresh();
            }
        }
    );
    impl ViewerTarget {
        fn refresh(&self) {
            let ivars = self.ivars();
            unsafe {
                let pages = ivars.document.pageCount();
                let page = ivars
                    .view
                    .currentPage()
                    .map(|page| ivars.document.indexForPage(&page) + 1)
                    .unwrap_or(1);
                ivars
                    .page_label
                    .setStringValue(&NSString::from_str(&format!("Page {page} of {pages}")));
                for (button, enabled) in ivars.buttons.iter().zip([
                    ivars.view.canGoToPreviousPage(),
                    ivars.view.canGoToNextPage(),
                    ivars.view.canZoomOut(),
                    ivars.view.canZoomIn(),
                    true,
                    true,
                ]) {
                    button.setEnabled(enabled);
                }
            }
        }
    }

    fn native_color(color: egui::Color32) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(color.r()) / 255.,
            f64::from(color.g()) / 255.,
            f64::from(color.b()) / 255.,
            1.,
        )
    }

    fn toolbar_button(
        mtm: MainThreadMarker,
        label: &str,
        symbol: Option<&str>,
        tag: isize,
        x: f64,
        width: f64,
    ) -> Retained<NSButton> {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(if symbol.is_some() { "" } else { label }),
                None,
                Some(sel!(viewerAction:)),
                mtm,
            )
        };
        button.setTag(tag);
        button.setFrame(NSRect::new(NSPoint::new(x, 12.), NSSize::new(width, 32.)));
        button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        button.setFont(Some(&NSFont::systemFontOfSize(12.)));
        button.setToolTip(Some(&NSString::from_str(label)));
        button.setAccessibilityLabel(Some(&NSString::from_str(label)));
        if let Some(symbol) = symbol {
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(label)),
            ) {
                button.setImage(Some(&image));
                button.setImagePosition(NSCellImagePosition::ImageOnly);
            } else {
                button.setTitle(&NSString::from_str(label));
            }
        }
        button
    }

    pub struct PdfWindow {
        pub window: Retained<NSWindow>,
        pub view: Retained<PDFView>,
        pub document: Retained<PDFDocument>,
        pub original_sha256: String,
        original_data: Retained<NSData>,
        toolbar: Retained<ViewerTarget>,
    }
    impl PdfWindow {
        /// Must be called on the UI thread. Retain this handle for the window's lifetime.
        pub fn open(bytes: &[u8], title: &str) -> Result<Self, String> {
            let mtm =
                MainThreadMarker::new().ok_or("PDF viewing must run on the macOS UI thread")?;
            if bytes.len() > 500_000_000 {
                return Err("Stored PDF exceeds viewer safety limit".into());
            }
            let data = NSData::with_bytes(bytes);
            let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
                .ok_or("Stored PDF could not be decoded")?;
            if unsafe { document.pageCount() } == 0 {
                return Err("Stored PDF has no pages".into());
            }
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 1100.0));
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Resizable
                        | NSWindowStyleMask::Miniaturizable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            let root = NSView::initWithFrame(NSView::alloc(mtm), frame);
            let toolbar_view = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0., 1044.), NSSize::new(900., 56.)),
            );
            toolbar_view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            let title_label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
            title_label.setFrame(NSRect::new(NSPoint::new(64., 18.), NSSize::new(394., 20.)));
            title_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            title_label.setFont(Some(&NSFont::systemFontOfSize(14.)));
            title_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            title_label.setToolTip(Some(&NSString::from_str(title)));
            toolbar_view.addSubview(&title_label);
            let page_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            page_label.setFrame(NSRect::new(NSPoint::new(566., 19.), NSSize::new(112., 18.)));
            page_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            page_label.setAlignment(NSTextAlignment::Center);
            page_label.setFont(Some(&NSFont::systemFontOfSize(12.)));
            toolbar_view.addSubview(&page_label);
            let mut buttons = vec![
                toolbar_button(mtm, "Previous page", Some("chevron.left"), 0, 482., 32.),
                toolbar_button(mtm, "Next page", Some("chevron.right"), 1, 522., 32.),
                toolbar_button(mtm, "Zoom out", Some("minus.magnifyingglass"), 2, 692., 32.),
                toolbar_button(mtm, "Zoom in", Some("plus.magnifyingglass"), 3, 732., 32.),
                toolbar_button(mtm, "Fit page", None, 4, 778., 100.),
            ];
            let thumbnails_button = toolbar_button(
                mtm,
                "Show or hide page thumbnails",
                Some("sidebar.left"),
                5,
                20.,
                32.,
            );
            thumbnails_button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxXMargin);
            buttons.push(thumbnails_button);
            let view = unsafe {
                PDFView::initWithFrame(
                    PDFView::alloc(mtm),
                    NSRect::new(NSPoint::new(0., 0.), NSSize::new(900., 1044.)),
                )
            };
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let thumbnails = unsafe {
                PDFThumbnailView::initWithFrame(
                    PDFThumbnailView::alloc(mtm),
                    NSRect::new(NSPoint::new(0., 0.), NSSize::new(132., 1044.)),
                )
            };
            thumbnails.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
            thumbnails.setHidden(true);
            unsafe {
                thumbnails.setPDFView(Some(&view));
                thumbnails.setThumbnailSize(NSSize::new(98., 132.));
                thumbnails.setMaximumNumberOfColumns(1);
                thumbnails.setAllowsMultipleSelection(false);
            }
            let toolbar = ViewerTarget::alloc(mtm).set_ivars(ViewerTargetIvars {
                view: view.clone(),
                document: document.clone(),
                page_label,
                title_label,
                buttons,
                thumbnails: thumbnails.clone(),
            });
            let toolbar: Retained<ViewerTarget> = unsafe { msg_send![super(toolbar), init] };
            for button in &toolbar.ivars().buttons {
                unsafe {
                    button.setTarget(Some(&toolbar));
                }
                toolbar_view.addSubview(button);
            }
            let separator = NSBox::initWithFrame(
                NSBox::alloc(mtm),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(900., 1.)),
            );
            separator.setBoxType(NSBoxType::Separator);
            separator.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            toolbar_view.addSubview(&separator);
            root.addSubview(&view);
            root.addSubview(&thumbnails);
            root.addSubview(&toolbar_view);
            unsafe {
                window.setReleasedWhenClosed(false);
                window.setContentMinSize(NSSize::new(650., 420.));
                view.setDocument(Some(&document));
                view.setAutoScales(true);
                let center = NSNotificationCenter::defaultCenter();
                for name in [
                    PDFViewPageChangedNotification,
                    PDFViewScaleChangedNotification,
                ] {
                    center.addObserver_selector_name_object(
                        &toolbar,
                        sel!(pdfViewChanged:),
                        Some(name),
                        Some(&view),
                    );
                }
                window.setContentView(Some(&root));
                window.setTitle(&NSString::from_str(title));
                window.center();
            }
            toolbar.refresh();
            let viewer = Self {
                window,
                view,
                document,
                original_sha256: format!("{:x}", Sha256::digest(bytes)),
                original_data: data,
                toolbar,
            };
            viewer.set_appearance(crate::theme::Appearance::Light);
            viewer.window.makeKeyAndOrderFront(None);
            Ok(viewer)
        }
        /// Change viewer chrome only; the original document and its pages stay intact.
        pub fn set_appearance(&self, appearance: crate::theme::Appearance) {
            let colors = appearance.palette();
            let name = NSString::from_str(match appearance {
                crate::theme::Appearance::Light => "NSAppearanceNameAqua",
                crate::theme::Appearance::Dark => "NSAppearanceNameDarkAqua",
            });
            let native_appearance = NSAppearance::appearanceNamed(&name);
            self.window.setAppearance(native_appearance.as_deref());
            self.window
                .setBackgroundColor(Some(&native_color(colors.paper)));
            let toolbar = self.toolbar.ivars();
            unsafe {
                self.view.setBackgroundColor(&native_color(colors.sidebar));
                toolbar
                    .thumbnails
                    .setBackgroundColor(Some(&native_color(colors.sidebar)));
            }
            toolbar
                .title_label
                .setTextColor(Some(&native_color(colors.ink)));
            toolbar
                .page_label
                .setTextColor(Some(&native_color(colors.muted)));
            for button in &toolbar.buttons {
                button.setContentTintColor(Some(&native_color(colors.green)));
            }
        }
        pub fn page_count(&self) -> usize {
            unsafe { self.document.pageCount() }
        }
        pub fn extracted_text(&self) -> String {
            unsafe { self.document.string() }
                .map(|s| s.to_string())
                .unwrap_or_default()
        }

        /// Exercise real AppKit actions and PDFKit notifications with a disposable,
        /// multi-page PDF. This changes only the viewer's page, zoom and geometry.
        pub fn verify_toolbar_interactions(&self) -> Result<serde_json::Value, String> {
            let pages = self.page_count();
            if pages < 2 {
                return Err("Toolbar proof requires at least two synthetic PDF pages".into());
            }
            let retained_sha = format!("{:x}", Sha256::digest(self.original_data.to_vec()));
            if retained_sha != self.original_sha256 {
                return Err("Viewer retained data differs from its original PDF".into());
            }
            let text = self.extracted_text();
            let frame = self.view.frame();
            for appearance in [
                crate::theme::Appearance::Dark,
                crate::theme::Appearance::Light,
            ] {
                self.set_appearance(appearance);
                if self.page_count() != pages
                    || self.view.frame() != frame
                    || self.extracted_text() != text
                    || format!("{:x}", Sha256::digest(self.original_data.to_vec())) != retained_sha
                {
                    return Err(
                        "PDF appearance changed retained document or viewer geometry".into(),
                    );
                }
            }
            let toolbar = self.toolbar.ivars();
            unsafe {
                let first = self
                    .document
                    .pageAtIndex(0)
                    .ok_or("First PDF page missing")?;
                self.view.goToPage(&first);
                self.view.layoutSubtreeIfNeeded();
                self.toolbar.refresh();
                if toolbar.buttons[0].isEnabled() || !toolbar.buttons[1].isEnabled() {
                    return Err("PDF first-page navigation state is incorrect".into());
                }
                toolbar.buttons[1].performClick(None);
                if toolbar.page_label.stringValue().to_string() != format!("Page 2 of {pages}") {
                    return Err("Next-page button did not update the real PDF page label".into());
                }
                toolbar.buttons[0].performClick(None);
                if toolbar.page_label.stringValue().to_string() != format!("Page 1 of {pages}") {
                    return Err("Previous-page button did not return to page one".into());
                }
                // Bypass our actions to prove scrolling/keyboard-driven PDFKit
                // navigation also reaches the notification-based page label.
                let last = self
                    .document
                    .pageAtIndex(pages - 1)
                    .ok_or("Last PDF page missing")?;
                self.view.goToPage(&last);
                if toolbar.page_label.stringValue().to_string()
                    != format!("Page {pages} of {pages}")
                    || toolbar.buttons[1].isEnabled()
                {
                    return Err("PDFKit page change did not update the toolbar boundary".into());
                }
                let before_zoom = self.view.scaleFactor();
                toolbar.buttons[3].performClick(None);
                if self.view.scaleFactor() <= before_zoom || self.view.autoScales() {
                    return Err("PDF zoom-in action did not change the native scale".into());
                }
                let zoomed = self.view.scaleFactor();
                toolbar.buttons[2].performClick(None);
                if self.view.scaleFactor() >= zoomed {
                    return Err("PDF zoom-out action did not change the native scale".into());
                }
                toolbar.buttons[4].performClick(None);
                if !self.view.autoScales() {
                    return Err("Fit page did not restore native automatic scaling".into());
                }
            }
            self.window.setContentSize(NSSize::new(650., 420.));
            self.view.layoutSubtreeIfNeeded();
            let frame = self.view.frame();
            if (frame.size.width - 650.).abs() > 1. || (frame.size.height - 364.).abs() > 1. {
                return Err("PDF view did not resize below its fixed-height toolbar".into());
            }
            unsafe {
                toolbar.buttons[5].performClick(None);
                if toolbar.thumbnails.isHidden()
                    || toolbar.thumbnails.PDFView().is_none()
                    || self.view.frame().origin.x != 132.
                    || (self.view.frame().size.width - 518.).abs() > 1.
                {
                    return Err(
                        "Page thumbnails did not open beside the exact PDF at narrow size".into(),
                    );
                }
                toolbar.thumbnails.layoutSubtreeIfNeeded();
                toolbar.buttons[5].performClick(None);
                if !toolbar.thumbnails.isHidden()
                    || (self.view.frame().size.width - 650.).abs() > 1.
                {
                    return Err("Hiding thumbnails did not restore PDF width".into());
                }
            }
            if self.extracted_text() != text
                || format!("{:x}", Sha256::digest(self.original_data.to_vec())) != retained_sha
            {
                return Err("PDF toolbar interaction changed retained document content".into());
            }
            Ok(serde_json::json!({
                "page_count": pages,
                "original_sha256": retained_sha,
                "appearance_cycle_preserves_bytes_text_pages_geometry": true,
                "native_previous_next": true,
                "independent_pdfkit_page_notification": true,
                "page_boundaries": true,
                "zoom_in_out_fit": true,
                "narrow_resize": true,
                "native_page_thumbnails_toggle": true,
                "retained_bytes_and_text_unchanged": true,
            }))
        }
    }
    impl Drop for PdfWindow {
        fn drop(&mut self) {
            // NSNotificationCenter does not own selector observers. Remove them
            // before the target and view leave this owner.
            unsafe {
                NSNotificationCenter::defaultCenter().removeObserver(&self.toolbar);
            }
        }
    }
    /// Derive a transient card image from the exact viewer PDF on its background
    /// owner. Neither the PDF nor the thumbnail is rewritten or stored.
    pub fn cover_thumbnail(
        bytes: &[u8],
        expected_sha256: &str,
        expected_pages: usize,
    ) -> Result<image::RgbaImage, String> {
        if bytes.is_empty() || bytes.len() >= 100_000_000 {
            return Err("Stored cover PDF exceeds the managed storage limit".into());
        }
        if format!("{:x}", Sha256::digest(bytes)) != expected_sha256 {
            return Err("Stored cover PDF hash mismatch".into());
        }
        objc2::rc::autoreleasepool(|_| {
            let data = NSData::with_bytes(bytes);
            let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
                .ok_or("Stored cover PDF could not be decoded")?;
            let pages = unsafe { document.pageCount() };
            if pages == 0 || pages != expected_pages {
                return Err("Stored cover PDF page count mismatch".into());
            }
            let page = unsafe { document.pageAtIndex(0) }.ok_or("Stored cover page missing")?;
            let bounds = unsafe { page.boundsForBox(PDFDisplayBox::MediaBox) };
            if !bounds.size.width.is_finite()
                || !bounds.size.height.is_finite()
                || bounds.size.width <= 0.0
                || bounds.size.height <= 0.0
                || bounds.size.width > 20_000.0
                || bounds.size.height > 20_000.0
            {
                return Err("Stored cover PDF has invalid paper geometry".into());
            }
            let thumbnail = unsafe {
                page.thumbnailOfSize_forBox(NSSize::new(680.0, 880.0), PDFDisplayBox::MediaBox)
            };
            // PDFKit owns rasterization. Encode its bounded bitmap as PNG so the
            // existing Rust decoder handles channel order and alpha correctly.
            let tiff = thumbnail
                .TIFFRepresentation()
                .ok_or("Native cover raster missing")?;
            if tiff.length() > 40_000_000 {
                return Err("Native cover raster exceeds memory limit".into());
            }
            let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)
                .ok_or("Native cover raster could not be decoded")?;
            if bitmap.pixelsWide() <= 0
                || bitmap.pixelsHigh() <= 0
                || bitmap.pixelsWide() > 2720
                || bitmap.pixelsHigh() > 3520
            {
                return Err("Native cover raster has invalid dimensions".into());
            }
            // Egui consumes sRGB bytes and the Rust PNG decoder does not apply
            // embedded ICC profiles. Convert native device/P3 pixels explicitly
            // before handing the raster to that boundary.
            let bitmap = bitmap
                .bitmapImageRepByConvertingToColorSpace_renderingIntent(
                    &NSColorSpace::sRGBColorSpace(),
                    NSColorRenderingIntent::Default,
                )
                .ok_or("Native cover sRGB conversion failed")?;
            let png = unsafe {
                bitmap.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }
            .ok_or("Native cover PNG conversion failed")?;
            if png.length() > 40_000_000 {
                return Err("Native cover PNG exceeds memory limit".into());
            }
            let png = png.to_vec();
            let mut reader = image::ImageReader::with_format(
                std::io::Cursor::new(&png),
                image::ImageFormat::Png,
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(2720);
            limits.max_image_height = Some(3520);
            limits.max_alloc = Some(40_000_000);
            reader.limits(limits);
            let image = reader
                .decode()
                .map_err(|error| format!("Native cover raster: {error}"))?;
            Ok(image.thumbnail(680, 880).to_rgba8())
        })
    }
    pub fn inspect_pdf(bytes: &[u8]) -> Result<(usize, String), String> {
        let data = NSData::with_bytes(bytes);
        let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
            .ok_or("Stored PDF could not be decoded")?;
        Ok((
            unsafe { document.pageCount() },
            unsafe { document.string() }
                .map(|s| s.to_string())
                .unwrap_or_default(),
        ))
    }
}
#[cfg(target_os = "macos")]
pub use macos::{PdfWindow, cover_thumbnail, inspect_pdf};

#[cfg(all(test, target_os = "macos"))]
mod cover_tests {
    use sha2::{Digest, Sha256};

    fn colored_pdf() -> Vec<u8> {
        // Two spatially distinct colors prove that real page content is drawn,
        // while allowing ColorSync's legitimate device-to-sRGB conversion.
        let stream = b"1 0 0 rg 0 0 50 100 re f 0 0 1 rg 50 0 50 100 re f";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> /Contents 4 0 R >>".to_vec(),
            format!("<< /Length {} >>\nstream\n{}\nendstream",stream.len(),String::from_utf8_lossy(stream)).into_bytes(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(object);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes(),
        );
        for offset in &offsets[1..] {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        pdf
    }

    #[test]
    fn exact_cover_raster_is_bounded_and_works_on_background_owner() {
        std::thread::spawn(|| {
            let bytes = colored_pdf();
            let original = bytes.clone();
            let sha = format!("{:x}", Sha256::digest(&bytes));
            let raster = super::cover_thumbnail(&bytes, &sha, 1).unwrap();
            assert!(raster.width() > 0 && raster.width() <= 680);
            assert!(raster.height() > 0 && raster.height() <= 880);
            let left = raster.get_pixel(raster.width() / 4, raster.height() / 2);
            let right = raster.get_pixel(3 * raster.width() / 4, raster.height() / 2);
            assert!(
                left[0] > 200 && left[1] < 80 && left[2] < 80 && left[3] > 240,
                "left red={left:?}"
            );
            assert!(
                right[2] > 200 && right[0] < 80 && right[1] < 80 && right[3] > 240,
                "right blue={right:?}"
            );
            assert_eq!(bytes, original);
            assert!(
                super::cover_thumbnail(&bytes, &"0".repeat(64), 1)
                    .unwrap_err()
                    .contains("hash mismatch")
            );
            assert!(
                super::cover_thumbnail(&bytes, &sha, 2)
                    .unwrap_err()
                    .contains("page count mismatch")
            );
        })
        .join()
        .unwrap();
    }
}
