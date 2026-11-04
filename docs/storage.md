# Storage

## App bundle under 50 MB, library unbounded

The only size budget is the app itself: `Reader.app` must stay strictly below
50,000,000 bytes. `packaging/macos/bundle.sh` measures every bundle it builds
and fails above the limit, and `scripts/dmg.sh` reports the disk image's size.

The library (SQLite and its WAL, settings, credentials, receipts, PDFs,
publication icons, WhatsApp state and the article cache) has no size limit.
Saves, imports, feed intake, magazine production and updates never fail because
the library is large. Writers still record in-flight work as reservations, and
each job is held to the size it declared, but nothing is refused for lack of
budget. Article metadata and finalized magazine PDFs are never evicted.
Updates download into the system temporary folder and only need free disk
space.

Full article bodies, raster images, and publication icons share a 20,000,000-byte
cache with seven-day expiry and least-recent-use eviction. Expired or missing
copies are fetched again on demand. They never change article quality or
reading state. Window and query settings live in `config/reader.json`.

## Measure an installed app

Measure the installed product without changing it:

```sh
'/Applications/Reader.app/Contents/MacOS/macos-read-later' storage-audit \
  --data-root '/path/to/your/library' \
  --bundle '/Applications/Reader.app'
```

This reports actual per-entry allocation, using the larger of file length and
allocated blocks. It rejects symlinks and exits unsuccessfully when the app
bundle is not below 50 MB. Library size is reported, never enforced.

## Move build output to Trash

Preview the Cargo output directories that would be moved:

```sh
./scripts/trash-builds.sh
```

Stop builds, then move those directories to Trash:

```sh
./scripts/trash-builds.sh --apply
```

The command checks Cargo cache tags and allowed top-level entries, rejects
symlinked targets and parents, and refuses to run while Cargo or rustc is
active. It preserves the library, toolchain, app bundles, and private evidence,
and records the Trash destination in `.local/trash-builds-*.txt`. Disk space
becomes available when you empty Trash.

## Audit and remove build output

Run the audit without changing files:

```sh
./scripts/storage.sh
./scripts/storage.sh clean-builds
```

Stop builds, then explicitly apply cleanup:

```sh
./scripts/storage.sh clean-builds --apply
```

The command removes only `target` and the proof targets under `.local/targets`.
It requires regular Cargo cache markers, rejects unexpected top-level entries,
refuses symlinked target directories or parents, and refuses to run while any
Cargo or rustc process is active. A cleanup lock prevents two maintenance
commands from running together. Toolchains, downloaded dependencies, databases,
packaged apps, and private evidence are preserved.

## Keep development storage bounded

Use separate target directories only while builds actually run concurrently,
and remove them afterwards. `scripts/verify.sh` runs the main checks
sequentially in the normal `target` tree. The main app and proof manifests use
line-table debug information and disable incremental compilation, which keeps
build trees small while preserving source-line backtraces.
