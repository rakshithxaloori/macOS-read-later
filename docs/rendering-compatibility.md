# Native rich content and PDF compatibility

The production reader uses Rust HTML parsing and an embedded AppKit `NSTextView`. Rust writes RTF for typography, tables, lists, code and links. Cached raster images are inserted as explicit `NSTextAttachment` objects after native RTF decoding; AppKit's RTF importer silently drops standard image groups, so those groups are not relied upon. The reader does not invoke the platform HTML importer, JavaScript, a webview, or publisher image requests. Missing cached images remain visible as unavailable notices.

The native document keeps the publication, title, metadata, typed article actions, copy notice and rich body in one scroll surface below the fixed toolbar. The native text views provide continuous selection, copy and external links. Winit's original view remains the window's content view, with the native reader attached as a child. A subclass with no additional instance variables composes AccessKit's controls and the native reader into one accessibility tree while preserving Winit's required memory layout. Native controls retain standard accessibility names. Its delegate resolves stored footnote anchors and scrolls to their native text range. Reader prose uses Georgia at the selected size, 1.85 line spacing and 1.3em paragraph spacing. Headings scale to 1.5em and tables adapt to the current width. The body adapts to the reference margins and a 780-point maximum width; attachments shrink and grow within the current body width.

PDF review passes the exact stored bytes to PDFKit. The viewer records their SHA-256 and permits historical masters up to 500 MB independently of new production's 50 MB bound. It does not regenerate a compressed or simplified review copy.

Historical editions keep only their viewer PDF; production masters and separate cover files are retired. Magazine cards derive bounded cover previews from the first page of the verified PDF in memory. These previews create no persistent thumbnails. The viewer's broader decode bound is compatibility code, not permission to exceed the shared 100 MB product budget.

The production renderer runs as an isolated child of the same Rust executable, with no networking. It uses one bounded text container per column per page. Native automatic `NSTextView` printing duplicated a line across a page boundary during the proof; explicit glyph ranges eliminate this error. Full-page front/back covers are printed separately and merged with the complete interior through PDFKit. Cover copy must fit the reserved top-left area. Letter/A4/A5 geometry, selected columns, fonts, spacing and theme colors are applied to the native layout. Data images receive more vertical space than illustrations. Corrected authors/dates and their source links appear separately from original snapshots.

Every PDF link annotation comes from its exact native character/glyph range. Repeated labels therefore retain their own URLs. Footnotes use PDF destinations and receive per-article anchor scopes. Verification selects each printed native line at its PDF glyph coordinates and compares the complete interior character inventory. PDFKit can reorder table cells and footers at a column boundary when extracting a whole page; the native layout supplies the exact reading order for these checks. Missing or duplicated content fails the inventory, and each phrase must occur at its printed position. Article endings, cached image attachments, source links and paper geometry are checked separately. Both print views override the default hardware imageable offset, keeping artwork and link rectangles aligned with the full paper. The Canva profile rejects pages outside 24–204, odd pages, non-Letter paper and one-column interiors; it does not truncate or add filler.

Run the independent native proof:

```sh
scripts/cargo-local.sh run --locked --manifest-path proofs/rendering/Cargo.toml \
  --target-dir .local/targets/rendering -- .local/rendering-proof
```

The synthetic proof verifies an actual native footnote callback, retained external-link attribute, raster attachment, two-column table cell coordinates and exact full PDF text. `--host-proof` also exercises the embedded whole-reader adapter at wide and narrow widths, its typed native button action, continuous selection and keyboard-focus restoration. `--show` leaves its native reader and PDFKit windows open for inspection.

Add `--retained` to verify your own library's cached articles and stored viewer PDFs, and `--render-job INPUT OUTPUT` to render a production job. Their output stays in ignored `.local` directories.

Use `--raster-pdf INPUT OUTPUT_DIRECTORY` to render representative cover, interior, companion and final pages through native PDFKit for visual inspection. These adapter and production proofs are separate from inspection of the assembled egui app, bundle lifecycle and a physical print. Live WhatsApp pairing is outside this proof.

RTF cell and row terminators explicitly retain table context after nested paragraph groups close, so adjacent cells share a row instead of stacking vertically. A native geometry regression covers this.

The art-enabled fixture also preserves linked author/date corrections, full-page painting geometry, cover derivative hashes and cached-media hashes. A parity companion reuses the approved front painting immediately before the back when necessary; an odd edition without an approved companion requires another whole-article selection. Companions do not satisfy Canva's minimum page count by adding filler. Inline images preserve their position between surrounding prose and retain their original external hyperlink.
