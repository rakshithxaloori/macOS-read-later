# Changelog

All notable changes to Read Later (formerly Reading List) are recorded here. Versions follow
[Semantic Versioning](https://semver.org): a major version for changes that need
a library conversion, a minor version for features and redesigns, and a patch
version for fixes. Each release is a `vX.Y.Z` tag on `main`.

## [Unreleased]

### Fixed
- RSS feeds hosted on WordPress.com (192.0.78.0/24) no longer fail with "URL
  resolves to a private network". Only 192.0.0.0/24 and 192.0.2.0/24 are
  treated as special-purpose.
- Full-text feeds no longer exhaust the storage budget on replay. Lean replay
  reserves space per entry rather than four times the XML size, so one large
  feed can no longer fill the shared replay buffer and block other feeds.

### Changed
- Renamed the app to Read Later. The bundle is `Read Later.app`, the crate and
  executable are `macos-read-later`, and the repository is `macOS-read-later`.
  The bundle identifier, Keychain service and portable export format keep their
  old identifiers so existing libraries and WhatsApp sessions keep working.

## [0.2.2] - 2026-10-28

### Fixed
- Reader toolbar hover labels open beside their buttons instead of below,
  where the article view hid them.

## [0.2.1] - 2026-10-28

### Changed
- Article rows no longer show a warning triangle. A failed copy says
  "Unavailable" and a pending one "Fetching" in the row's metadata line;
  partial copies are explained in the reader.

## [0.2.0] - 2026-10-26

### Added
- Versioned releases: the bundle version comes from `Cargo.toml`, the build
  number from the commit count, and **Reading List → About Reading List** shows
  both. `scripts/release.sh X.Y.Z` verifies, tags, bundles and archives a release.
- A new app icon: an ivory open book with a copper bookmark on deep green.
  `packaging/macos/icon/make-icon.sh` regenerates it.

### Changed
- One continuous window: the panes run under a transparent title bar.
- Inbox and Archive header fits on one line: title and count, an Unread/Read
  segmented control, and search with a filter-and-sort popover.
- Rows show the publication name and reading time; only failed copies use the
  warning color.
- Reader actions (re-fetch, mark read, archive, delete, open original) moved
  from inside the article to toolbar icons. Reader line height is now 1.6.
- Feeds are compact single rows, so about twice as many fit on screen.
- Warmer, deeper Dark appearance; the rail's "Yours." label is gone.

### Removed
- One-time migration, compaction and cutover commands (`migrate`, `compact`,
  `cutover`) and their tests. Libraries still in the retired full-copy format
  must be converted with v0.1.0 first.

### Fixed
- The app icon no longer turns blank when the window is closed or the app quits;
  the bundle now ships `AppIcon.icns`.
- The Magazines heading read "FROM SCREEN TO colors.paper".

## [0.1.0] - 2026-10-22

First versioned baseline.

- Native Rust reader (egui, AppKit text views, PDFKit) with Inbox, Archive,
  Feeds, Magazines, WhatsApp capture and Import/export.
- RSS/Atom intake keeps older articles when a feed's window slides, with
  15-minute polling and crash-safe replay.
- WhatsApp self-chat `save <link>` capture with fresh native pairing.
- Lean storage: app plus all managed data stays under 100 MB, with a seven-day,
  20 MB on-demand article cache (Oct 18).
- Reader font picker: Georgia, Baskerville, Avenir Next, Helvetica Neue (Oct 18).
- Intake resumes correctly after sleep (Oct 21).
- Read Inbox rows fade instead of showing status dots (Oct 21).
- Light and Dark appearance, shared control family, vector icons, clearer reader
  recovery and a visual magazine workflow (Oct 22).
