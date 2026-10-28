# Native magazines

Magazines keep ordered frozen drafts and exact stored-PDF review. In a converted
library, historical editions retain their exact viewer PDF, edition metadata,
checksums, and compact provenance. Their article bodies, print masters, artwork,
cover originals, and source-repair bytes are intentionally retired. They are
immutable viewers rather than editable production archives.

The planner considers the entire otherwise-eligible unread library, excluding
archived, deleted, read, excluded, drafted, and printed articles. With metadata-only
articles it acquires publisher content, measures full word counts, and shows
progress and unavailable candidates. Complete measured candidates feed deterministic
proposals of 3–13 whole articles with at most two per normalized publication and
an approximate 18,225-word target. It never invents summaries or truncates articles.
Manual drafts retain your ordered selection.

Creating or explicitly refreshing a draft acquires full copies outside the
snapshot write transaction. A transaction freezes original title, URL, author,
HTML, quality, word count, and acquisition hashes/times. Draft-owned raster images
live in `native_magazine_media`, independently of the reader cache. Archiving an
article or evicting cache cannot remove a draft's frozen content. Reordering and
presentation edits retain existing snapshots; explicit refresh replaces them and
reclaims unreferenced media. Any draft edit invalidates its prior PDF/proof and
review acknowledgement.

Theme Studio preserves Signal & Paper, Literary Review, Swiss Journal, and Modern
Review; Georgia, Baskerville, Avenir, and Helvetica; Letter, A4, and A5; columns,
typography, colors, and print profiles. New-issue bounds remain 40 articles,
60,000 words, 20 MB snapshot text, 50 MB media, 50 MB PDF, 400 pages, and five
minutes of rendering. The global installed-app/data budget is stricter: these
individual maxima are not simultaneous allowances. The Canva profile requires
Letter, two columns, and 24–204 even pages. Imported historical viewers are not
subjected retroactively to new-draft bounds.

Author/date corrections and recovered images are separate presentation data.
Image repair requires an exact unique paragraph anchor, alt text, caption,
source page and image URLs, retrieval time, and SHA-256 matching its retained
original. Generation uses a separate presented copy; original snapshots remain
unchanged while the draft exists.

The cover workflow preserves seven versioned prompts in `prompts/covers` and a
fingerprint of the current draft and prompt inputs. Export the request, generate
a text-free front painting through Codex, inspect it, then reference it when
creating the back. Reserve the requested ivory area for overlaid text. Import
both originals into the matching draft. No unattended paid image API runs.
Fulfilment validates static PNG/JPEG bytes and decoding bounds, rejects stale
fingerprints or changed repeat fulfilment, and stores originals separately from
print derivatives with their hashes and provenance. Cover overflow is an error.

A dedicated worker claims explicitly queued jobs with random-token leases and
heartbeats. It evicts disposable reader cache before reserving serialized input,
intermediate/final PDFs, verification output, and SQLite/WAL publication space
under the same 100,000,000-byte ceiling. If the complete edition cannot fit, it
fails with a storage error. No prose or required images are discarded to fit.

The same Rust executable runs `render-magazine` in an isolated main-thread
AppKit/PDFKit child using production-owned media and no network fetching. Native
PDF output is created in memory before bounded filesystem writes. The owner
checks file growth, enforces the five-minute deadline, stops the child on Quit,
and recovers expired workspaces after restart. Completed, failed, and interrupted
temporary files are removed; bounded diagnostics remain in queue state. Stale
completion cannot publish a PDF.

A completed proof checks text, endings, openings, images, links, geometry, and
cover copy before publication. Review opens the exact stored PDF bytes through
PDFKit. Acknowledge review of that checksum before finalizing. In a lean library,
finalization preserves that PDF and compact acquisition/artwork/repair provenance,
then deletes snapshot HTML, production-owned images, cover originals, and other
production payloads before freezing the edition. Article read/archive state stays
unchanged. Drafts keep their frozen inputs until finalized or explicitly deleted.

New finalized PDFs consume durable space. The app never automatically evicts
them; unlimited editions cannot fit a fixed budget. For a future native finalized
edition, **Export PDF and free local space** shows an explicit confirmation, then
opens a native Save PDF dialog. Cancel leaves its PDF intact. The export must be
outside managed library storage and the app bundle. Existing files are never
overwritten; an existing exact matching PDF can support an interrupted retry.

The background owner writes and syncs the exact PDF, verifies its length and
SHA-256, then records an immutable bounded export receipt and removes the local
PDF in one SQLite transaction. It reserves deletion-WAL space and reclaims free
pages afterward. Edition metadata, compact provenance, original viewer checksum,
and article references remain, so exported articles do not re-enter proposals.
The edition shows its exported status and path instead of claiming a local PDF is
ready. Keep the external file; the app does not promise to recreate it. Imported
historical viewer PDFs remain protected. Immutable origin and
receipt guards prevent treating them as disposable new native output or silently
restoring an exported PDF.

Synthetic coverage is in `tests/magazine.rs`, `tests/lean_magazines.rs`, and module
unit tests.
Live cover generation remains user-driven, and physical print inspection is distinct from PDF verification.
