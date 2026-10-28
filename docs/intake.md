# Native intake

`src/intake.rs` owns RSS and the durable article/feed job queue on a named
background thread with a current-thread Tokio runtime. Intake starts disabled,
is enabled after normal startup, and remains disabled under `--offline`.
Closing the window keeps its handle alive. Explicit Quit cancels in-flight
network work, requeues interrupted owned jobs, and joins the worker.

The 32-slot command channel accepts Enable, Wake, and Quit. Bounded events wake
the UI after publication. Saving a link, changing a feed, importing, or requesting
a refresh wakes the worker. An exclusive private `intake.lock` plus a random
process token prevents competing intake owners. SQLite process and job leases
renew during downloads; every publication checks the tokens and deadlines.
Network awaits never run inside write transactions.

## Converted-library retention

RSS and Atom polling runs every fifteen minutes for active subscriptions. The
worker prioritizes pending captures before fetching more feeds. It commits received
XML before parsing, with a shared 5,242,880-byte pending-XML bound and a storage
reservation. The article/source updates and successful capture removal commit
together. A crash cannot acknowledge discarded arrivals before they are durable.
Pausing, unsubscribe, cutoff, or smaller feed windows never delete prior articles.

RSS saves the title, author, dates, and bounded preview without scheduling every
publisher page for permanent extraction. A preview-only arrival can remain partial;
the foreground reader determines usable content when it fetches the article.
Duplicates preserve reading/archive/deletion state. RSS replay cannot resurrect
tombstones, and deliberate saving remains separate from RSS arrival identity.

`native_feed_state` keeps the latest content hash and bounded ETag/Last-Modified
validators instead of permanent XML history. Conditional requests can return 304
without another capture. Feed parsing rejects DTD/entity declarations and limits
entries to 5,000. Unsupported text encodings report a diagnostic.

Manual and WhatsApp saves can fetch a page for metadata, then discard its source
bytes, full HTML, and images. Repeated or weaker refreshes preserve useful metadata.
The lean schema rejects permanent article bodies/media. Completed intake-attempt
diagnostics retain the latest 100 outcomes; pending jobs, retryable work, source
identities, and acknowledgement receipts remain durable. Five failed attempts
stop automatic retries for a job.

The shared storage owner reserves room for metadata and XML writes. Successful
processing requests WAL truncation and incremental page reclamation outside the
write transaction. Busy readers defer that maintenance without replaying already
committed arrivals. Budget errors are reported instead of growing an unbounded
capture archive.

## Foreground reading

`src/loader.rs` owns article-on-click acquisition independently of the intake
queue. The UI shows title/preview metadata immediately; the loader supplies
sanitized prose before image downloads, then a final native document with bounded
private raster bytes. Request generations fence results and errors; navigation
cancels obsolete network futures. Archive/delete invalidation prevents a late
fetch from repopulating purged cache. Archived article views stay in memory
rather than re-entering persistent cache.

The disposable cache is limited to 20,000,000 bytes with seven-day expiry and
least-recent-use eviction. Cache absence is separate from extraction quality.
Metadata search and read/archive state do not depend on cache contents. A failed
uncached load retains metadata, offers Retry/Open original, and does not claim a
permanent offline copy.

Publisher and image requests use `network.rs` for public-address DNS validation,
address pinning, per-redirect checks, a 25-second timeout, and a 5 MB response
limit. The reader bounds image attempts and decoded raster allocations. Rich
rendering consumes downloaded private bytes and does not independently fetch
publisher images.

## Verification

`tests/intake.rs` covers the earlier retained-copy implementation, leases, replay,
and extraction with disposable SQLite databases and synthetic fetchers.
`tests/lean_intake.rs` adds the revised metadata-only and capture-reclamation
contract. Loader and storage tests cover foreground/cache behavior separately.
These tests do not contact publishers, pair phones, or send WhatsApp messages.
