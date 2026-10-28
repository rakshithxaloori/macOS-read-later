# Native application ownership

Views emit typed actions after drawing and send bounded commands to one database
owner. Metadata loads in 60-row pages. Query and reader results carry request
generations so stale searches or navigation cannot replace the current view.
Inbox and Archive retain independent preferences and scroll positions.

The database owner runs explicit SQLite transactions with compatible SQL
functions, WAL, foreign keys, FULL synchronization, and a 10-second busy timeout.
Versioned lean-schema guards prohibit permanent article HTML/media. Metadata,
previews, states, tombstones, subscriptions, source identities, receipts, and
provenance remain durable. Search uses metadata consistently rather than a
changing collection of cached full bodies.

The foreground loader has its own background Tokio runtime. It checks a bounded
cache or fetches, extracts, and sanitizes a publisher page, sends prose first,
then supplies private raster bytes to native rich rendering. Navigation cancels
obsolete work. Archive/delete increments a cache generation and purges owned
payloads, preventing late writes from recreating an archived copy. An archived
article can remain in memory for the current view.

`storage.rs` coordinates cross-process disk reservations with locked claim files.
It counts the installed bundle and every managed file, including filesystem
allocation, WAL, credentials, settings, receipts, and temporary work. The ceiling
is strictly below 100,000,000 bytes with 2 MB safety headroom. The 20 MB article
cache uses atomic publication, seven-day expiry, least-recent-use eviction, and
orphan/claim recovery. Durable metadata and finalized PDFs are never auto-evicted.
SQLite page limits, short WAL checkpoints, and incremental reclamation supplement
reservations. Developer Cargo/toolchain directories are separate from this budget.

RSS intake persists only bounded unprocessed XML, atomically records arrivals and
removes successfully processed captures, and keeps compact hash/HTTP validators.
Manual/WhatsApp extraction updates metadata without retaining full copies.
Network work uses Tokio outside transactions. Durable queues and token leases
recover after crashes; stale tokens cannot publish results.

Magazines acquire content independently of the reader cache. A draft transaction
freezes ordered content and production-owned media. A leased worker reserves its
complete temporary working set, then invokes the same Rust executable in a
main-thread AppKit/PDFKit child without network fetching. Finalized lean editions
retain exact viewer PDF bytes, immutable metadata, and compact provenance while
retiring production payloads. PDFKit displays the exact stored bytes for review.

WhatsApp has an independent encrypted native credential store and starts
disconnected. Verified outgoing self-chat events become durable arrivals and
acknowledgement receipts.

The window and application have separate lifetimes. Closing hides the reader;
native menus reopen it. Explicit Quit cancels and joins workers before process
exit. Menus and native framework calls remain behind macOS modules.
