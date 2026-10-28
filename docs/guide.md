# Read Later user guide

Read Later is a local, single-user macOS reader written in Rust. It uses
egui/eframe, Tokio, SQLite, AppKit text views, and PDFKit. Its runtime contains
no website, JavaScript, Node, Chromium, WebView, or Poppler helper.

It combines a read-later inbox (like Matter) with an RSS reader (like
NetNewsWire). Feeds keep older articles when a publisher's feed window slides,
and links sent to your own WhatsApp chat as `save <link>` land in the Inbox.
The installed app plus all managed data stays under 100 MB; full articles are
fetched on demand.

See [CHANGELOG.md](../CHANGELOG.md) for what changed in each version.

## Open the reader

Build the app with `./packaging/macos/bundle.sh` and open
`.local/dist/Read Later.app`, or copy it to `/Applications` first.

By default the library lives in the app's macOS Application Support directory,
with `config/` and `cache/` inside it. To use a different library, pass its
directory explicitly. `--data` takes precedence over the default. Add
`--offline` to inspect a library with network intake disabled:

```sh
'.local/dist/Read Later.app/Contents/MacOS/macos-read-later' --data /absolute/path/to/library --offline
```

## Use your library

In a converted library, opening an article shows its stored title and preview,
then fetches its full rich content when no valid cached copy exists. The shared
20,000,000-byte cache expires copies after seven days and evicts older entries
when space is needed. Only cached copies work offline; publishers can change,
remove, or restrict articles. Retry and Open original remain available on errors.

Opening preserves unread state. Archive marks an article read and purges its
cached text and images. An archived article can be fetched for the current view
without saving another persistent copy. Restore preserves the current read state.
Inbox and Archive retain separate search, filters, sorting, grouping, and scroll.
Search matches metadata, preview, author, publication, and URL, independently of
which full articles happen to be cached.

Use **Save link** or Command-N to save a URL. Use **Your feeds** to add, pause,
sync, or unsubscribe from RSS and Atom publications. Network work runs in the
background. RSS stores metadata and previews; it does not download every full
article. Successful processing removes raw feed XML, while bounded unprocessed
captures remain available for crash replay. Reader images are downloaded by its
background loader and passed to native rendering as private bytes.

Closing the reader window keeps intake running. Open the app again or choose
**Read Later → Show Read Later** to reopen it. Choose **Quit Read Later**
or Command-Q to stop workers and exit. `--offline` disables network intake.

To pair WhatsApp, launch without `--offline`, open **WhatsApp**, enter your expected phone number, and scan the fresh native QR.
In your outgoing self-chat, send `save https://example.com/article`.
Bare links, other chats, historical messages, and commands with multiple links
are ignored. **Disconnect** removes native credentials and preserves arrivals.
Live pairing and delivery remain user-assisted verification checks.

## Import and export

Choose **Import / export → Choose import file**. Review the preview, then start
the import. Supported files include Matter CSV or ZIP, OPML, Netscape bookmark
HTML, URL CSV, and portable JSON or ZIP. Durable 25-row batches survive restart.
Retry failed rows from the run's progress card; successful rows remain intact.

Choose **Export complete library** to save the retained library data in a ZIP.
Converted libraries export article metadata, feeds, provenance, exact viewer PDFs,
and any still-active draft production artifacts. Temporary article-cache contents
and retired historical originals are excluded. The manifest records this retention
policy and each article's body availability. Exports exclude native credentials
and session keys. Importing rich source files into a converted library retains
their metadata, not permanent article bodies or media.

## Produce an edition

Choose **Magazines → Suggest magazines** for deterministic proposals, or select
articles from Inbox in the desired order. Proposals acquire and measure eligible
publisher content and show unavailable candidates explicitly. Open the draft to
change its order, title, paper size, theme, typefaces, columns, and sourced
presentation corrections.
Refreshing copies explicitly replaces snapshots and invalidates the proof.

For painted covers, export the versioned cover request. Use its Codex-assisted
workflow to make text-free front and back originals. Import the originals into
the matching draft. Drafts retain originals separately from print derivatives.
It does not call an unattended paid image API. Drafts own their snapshots and
images separately from the disposable article cache.

Generate the PDF, open **Review exact stored PDF**, inspect it, and acknowledge
that review before finalizing. In converted libraries, finalization retains the
exact PDF and compact provenance, then retires snapshot text and production
originals. Existing finalized editions become immutable PDF-only viewers. New
production reserves its full working space within the same 100 MB budget and
refuses an edition that cannot fit completely. It preserves article read and
archive state.
For a future native finalized issue, **Export PDF and free local space** first
asks you to confirm, then choose an external PDF location. The app verifies the
export before removing its local PDF. Edition metadata, checksums, provenance,
and printed-article references remain. Imported historical viewer PDFs
are protected from this action. Keep your exported file; the local app no longer
holds that PDF after removal.
See [magazine behavior and limits](magazines.md).

## Preserve and recover your data

The library directory contains `library.sqlite3`, private native credentials,
intake ownership files, and temporary production work. Settings are written
atomically to `config/reader.json`. Keep the library's active WAL with
its database, or use SQLite's online backup API for a consistent backup.

The lean storage owner counts the installed bundle, SQLite and WAL, credentials,
settings, PDFs, cache, receipts, and temporary production work. It reserves space
before managed writes and evicts disposable cache first. Metadata and finalized
PDFs are never automatically evicted. Storage exhaustion stops the operation with
an error. Future native issues offer the explicit verified export-and-remove
action when more space is needed.
Developer toolchains and Cargo targets are outside this product budget.

Measure the installed product without changing it:

```sh
'/Applications/Read Later.app/Contents/MacOS/macos-read-later' storage-audit \
  --data-root '/path/to/your/library' \
  --bundle '/Applications/Read Later.app'
```

The report counts allocated files, including the database WAL and receipts, and
fails at or above the limit. Cache and work can change the total during use.

## Releases

`Cargo.toml` holds the version. To cut a release from a clean `main`:

1. Move the `Unreleased` notes in `CHANGELOG.md` under a new `## [X.Y.Z] - date`
   heading and set `version = "X.Y.Z"` in `Cargo.toml`. Commit both.
2. Run `scripts/release.sh X.Y.Z`. It runs the full verifier, tags `vX.Y.Z`,
   builds the signed bundle in `.local/dist`, and writes
   `.local/releases/Read-Later-X.Y.Z.zip` with its SHA-256.
3. Install into Applications only after reviewing that build.

The bundle's short version is the Cargo version and its build number is the
commit count, both visible in **Read Later → About Read Later**.

## Build and verify

Install Rust with rustup; `rust-toolchain.toml` pins 1.98. `scripts/cargo-local.sh`
uses an isolated toolchain in ignored `.local/toolchain` when one is present.
Dependencies are locked in `Cargo.lock`.

Development and test builds retain line tables for backtraces and disable
incremental state to limit disk use. Separate proof targets isolate concurrent
builds; remove their generated output when verification is complete. See
[storage audit and cleanup](storage.md) for the repeatable commands.

```sh
./scripts/verify.sh
./packaging/macos/bundle.sh
```

The verifier runs formatting, strict Clippy, all synthetic tests, and the release
build sequentially. It writes private logs under `.local/verification`.
Packaging creates and verifies a signed local `.app` with this app's own identity.

Synthetic tests, native rendering proofs, assembled UI checks, live phone
pairing, and physical print review are distinct evidence. See [native ownership](architecture.md),
[rendering proofs](rendering-compatibility.md), and
[WhatsApp compatibility](whatsapp-compatibility.md).

### Application appearance

Use the moon/sun control at the bottom of the navigation rail to switch between
Light and Dark. The choice persists across restarts; existing settings default
to Light. Application colors live in `src/theme.rs` and also style the native
article reader. Magazine themes and printed PDFs keep their own colors.
