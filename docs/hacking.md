# Changing Read Later with a coding agent

This page is for you and for the coding agent you work with (Claude Code,
Codex, Cursor, or anything else that can read files and run commands). Ask the
agent to read it before making changes.

## The one-minute map

Read Later is a single Rust binary. The interface is egui; article text and
PDFs are drawn with AppKit and PDFKit.

| Area | Where |
| --- | --- |
| App startup, command-line flags | `src/main.rs`, `src/paths.rs` |
| Window, screens, navigation | `src/app.rs`, `src/ui/` (`library.rs` is the Inbox and Archive, `reader.rs` the article view, `secondary.rs` the Feeds screen) |
| Colours, spacing, fonts | `src/theme.rs`, `src/reader_style.rs`, `src/ui/controls.rs` |
| Database and every write | `src/backend.rs`, `src/library.rs`, `src/db_compat.rs` |
| RSS, saving links, background work | `src/intake.rs`, `src/network.rs` |
| Fetching and cleaning article pages | `src/loader.rs`, `src/extraction.rs`, `src/rich_content.rs` |
| WhatsApp saving | `src/whatsapp.rs`, `src/ui/whatsapp.rs` |
| Magazines and PDFs | `src/magazine.rs`, `src/production_renderer.rs`, `src/native_pdf.rs` |
| Import and export | `src/imports.rs`, `src/exports.rs` |
| The 100 MB storage budget | `src/storage.rs`, [storage.md](storage.md) |

[architecture.md](architecture.md) explains how these talk to each other.

## Commands

```sh
cargo run --release                  # run from source
cargo run --release -- --offline     # run without any network activity
./scripts/verify.sh                  # formatting, Clippy, all tests, release build
./packaging/macos/bundle.sh          # build .local/dist/Read Later.app
./scripts/dmg.sh                     # build a shareable .local/releases/Read-Later-X.Y.Z.dmg
```

The first build downloads the pinned Rust toolchain and compiles SQLCipher, so
it takes a few minutes. Later builds are quick.

## Rules that keep the app working

- Run `./scripts/verify.sh` before you call a change done. It must pass with no
  Clippy warnings.
- Tests use synthetic data only. Never add a test that contacts a real
  publisher, WhatsApp, or the network.
- Do not store full article text or images permanently. Only metadata is
  permanent; bodies go through the cache in `src/storage.rs`. This is how the
  app stays under 100 MB.
- Database changes go through the single owner in `src/backend.rs`. Do not open
  extra write connections from the UI.
- Never commit a library, a `.sqlite3` file, WhatsApp credentials, or anything in
  `.local/`. The `.gitignore` already excludes them.
- If you change behaviour a user can see, add a line under `## [Unreleased]` in
  `CHANGELOG.md`.

## Where your data lives

A bundle built from a fresh clone keeps its library in
`~/Library/Application Support`. Pass `--data /some/folder` to try a change
against a separate library, so experiments never touch the one you read from.
