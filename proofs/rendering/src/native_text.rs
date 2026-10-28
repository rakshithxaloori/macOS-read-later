use objc2::{
    AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSApplication, NSAttributedStringAppKitDocumentFormats, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSColor, NSLayoutManager, NSPrintInfo, NSPrintJobSavingURL,
    NSPrintOperation, NSPrintSaveJob, NSPrintingPaginationMode, NSScrollView, NSTextContainer,
    NSTextStorage, NSTextView, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSCopying, NSData, NSPoint, NSRange, NSRect, NSSize,
    NSString, NSURL,
};
use std::time::Instant;

struct PrintIvars {
    _storage: Retained<NSTextStorage>,
    manager: Retained<NSLayoutManager>,
    containers: Vec<Retained<NSTextContainer>>,
    columns: usize,
    column_width: f64,
    gap: f64,
    page_height: f64,
    page_count: usize,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind=MainThreadOnly]
    #[ivars=PrintIvars]
    #[name="ReadingListProofPrintView"]
    struct ProofPrintView;
    impl ProofPrintView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self)->bool {true}
        #[unsafe(method(knowsPageRange:))]
        fn knows_page_range(&self,range:*mut NSRange)->bool {if range.is_null(){false}else{unsafe{*range=NSRange::new(1,self.ivars().page_count);}true}}
        #[unsafe(method(rectForPage:))]
        fn rect_for_page(&self,page:isize)->NSRect {let v=self.ivars();let y=page.saturating_sub(1)as f64*v.page_height;NSRect::new(NSPoint::new(0.0,y),NSSize::new(v.column_width*v.columns as f64+v.gap*(v.columns-1)as f64,v.page_height))}
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self,_dirty:NSRect) {
            let v=self.ivars();let page=NSPrintOperation::currentOperation(self.mtm()).map(|o|o.currentPage().saturating_sub(1)as usize).unwrap_or(0);
            for column in 0..v.columns {if let Some(container)=v.containers.get(page*v.columns+column){let range=v.manager.glyphRangeForTextContainer(container);let origin=NSPoint::new(column as f64*(v.column_width+v.gap),page as f64*v.page_height);v.manager.drawBackgroundForGlyphRange_atPoint(range,origin);v.manager.drawGlyphsForGlyphRange_atPoint(range,origin);}}
        }
    }
);
impl ProofPrintView {
    fn create(
        attributed: &NSAttributedString,
        width: f64,
        height: f64,
        columns: usize,
        gap: f64,
        deadline: Instant,
        mtm: MainThreadMarker,
    ) -> Result<Retained<Self>, String> {
        let storage = NSTextStorage::new();
        storage.setAttributedString(attributed);
        let manager = NSLayoutManager::new();
        storage.addLayoutManager(&manager);
        let column_width = (width - gap * (columns - 1) as f64) / columns as f64;
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
                NSSize::new(column_width, height),
            );
            container.setLineFragmentPadding(0.0);
            manager.addTextContainer(&container);
            manager.ensureLayoutForTextContainer(&container);
            let range = manager.glyphRangeForTextContainer(&container);
            if range.length == 0 {
                return Err("A saved structure cannot fit the selected paper and columns".into());
            }
            covered = range.location + range.length;
            containers.push(container);
        }
        let page_count = containers.len().div_ceil(columns).max(1);
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, height * page_count as f64),
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
        });
        Ok(unsafe { msg_send![super(this),initWithFrame:frame] })
    }
}

pub fn decode_rtf(rtf: &[u8]) -> Result<Retained<NSAttributedString>, String> {
    let data = NSData::with_bytes(rtf);
    unsafe {
        NSAttributedString::initWithRTF_documentAttributes(NSAttributedString::alloc(), &data, None)
    }
    .ok_or("Native AppKit RTF decode failed".into())
}
pub struct NativeArticleWindow {
    pub window: Retained<NSWindow>,
    pub text: Retained<NSTextView>,
}
impl NativeArticleWindow {
    pub fn open(rtf: &[u8]) -> Result<Self, String> {
        let attributed = decode_rtf(rtf)?;
        Self::open_attributed(&attributed)
    }
    pub fn open_attributed(attributed: &NSAttributedString) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Native article needs main thread")?;
        let _app = NSApplication::sharedApplication(mtm);
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(880.0, 900.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                bounds,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), bounds);
        let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), bounds);
        unsafe {
            text.setEditable(false);
            text.setSelectable(true);
            text.setRichText(true);
            text.setVerticallyResizable(true);
            text.setHorizontallyResizable(false);
            text.setMinSize(NSSize::new(0.0, 900.0));
            text.setMaxSize(NSSize::new(880.0, f64::MAX));
            text.setTextContainerInset(NSSize::new(50.0, 35.0));
            text.textStorage()
                .ok_or("Native text storage missing")?
                .setAttributedString(attributed);
            if let Some(container) = text.textContainer() {
                container.setWidthTracksTextView(true);
                container.setContainerSize(NSSize::new(780.0, f64::MAX));
            }
            text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            scroll.setHasVerticalScroller(true);
            scroll.setDocumentView(Some(&text));
            text.setBackgroundColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                1.0,
                254.0 / 255.0,
                251.0 / 255.0,
                1.0,
            ));
            window.setReleasedWhenClosed(false);
            window.setTitle(&NSString::from_str(
                "Read Later • Native rich content proof",
            ));
            window.setContentView(Some(&scroll));
            window.center();
            window.makeKeyAndOrderFront(None);
        }
        Ok(Self { window, text })
    }
    pub fn pdf(&self, paper: NSSize) -> Result<Vec<u8>, String> {
        let info = NSPrintInfo::sharedPrintInfo().copy();
        {
            info.setPaperSize(paper);
            info.setLeftMargin(46.0);
            info.setRightMargin(46.0);
            info.setTopMargin(46.0);
            info.setBottomMargin(46.0);
            info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
            info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
            info.setVerticallyCentered(false);
        }
        let storage = unsafe { self.text.textStorage() }.ok_or("Native text storage missing")?;
        let view = ProofPrintView::create(
            &storage,
            paper.width - 92.0,
            paper.height - 92.0,
            1,
            0.0,
            Instant::now(),
            MainThreadMarker::new().ok_or("Main thread required")?,
        )?;
        let path = std::env::temp_dir().join(format!(
            "reading-list-native-proof-{}.pdf",
            std::process::id()
        ));
        let ns_path = NSString::from_str(&path.to_string_lossy());
        let saving_url = NSURL::fileURLWithPath(&ns_path);
        unsafe {
            info.setJobDisposition(NSPrintSaveJob);
            info.dictionary()
                .setObject_forKey(&saving_url, ProtocolObject::from_ref(NSPrintJobSavingURL));
        }
        let operation = NSPrintOperation::printOperationWithView_printInfo(&view, &info);
        operation.setShowsPrintPanel(false);
        operation.setShowsProgressPanel(false);
        if !operation.runOperation() {
            return Err("Native PDF production failed".into());
        }
        let data = std::fs::read(&path).map_err(|e| e.to_string())?;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(data)
    }
}
