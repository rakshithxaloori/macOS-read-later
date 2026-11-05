# Changelog

All notable changes to Reader (formerly Read Later and Reading List) are recorded here. Versions follow
[Semantic Versioning](https://semver.org): a major version for changes that need
a library conversion, a minor version for features and redesigns, and a patch
version for fixes. Each release is a `vX.Y.Z` tag on `main`.

## [Unreleased]

## [0.3.5] - 2026-11-05

### Added
- Grouped publications have a redesigned row with an initial tile, a count and
  a hover menu with "Archive all articles in this publication" and "Delete
  all…". The same archive action is in the list header menu inside a
  publication or feed.
- Clicking a feed in Feeds opens all of that feed's articles, read or unread,
  archived or not.

### Fixed
- Links in the reader show the pointing-hand cursor.

## [0.3.4] - 2026-11-04

### Changed
- The library has no size limit. Saving, importing, feed intake, magazine
  production and updates no longer fail with "Storage budget exhausted". The
  only size budget is the app: `Reader.app` must stay below 50 MB, and
  `bundle.sh` fails a bundle that is not. `storage-audit` reports library size
  and checks only the bundle.

### Fixed
- Install and Relaunch failed with "Storage budget exhausted" on libraries
  near the old 100 MB limit. Updates now download into the system temporary
  folder, check free disk space instead, and clean up on success and failure.
- Reader no longer refuses to start when the app and library together pass
  100 MB, and the library database no longer stops growing at 26.5 MB.

### Upgrading
- Reader 0.3.3's updater can fail with "Storage budget exhausted", so 0.3.3
  users may need to download 0.3.4 once by hand. Updates work from 0.3.4 on.

## [0.3.3] - 2026-11-03

### Added
- "Check for Updates…" in the app menu, and a daily launch check that can be
  turned off with "Check for Updates Automatically". When a newer release
  exists, a banner offers Install and Relaunch, Later, or Skip This Version.
  Install downloads the release's `Reader.dmg` (or `Read-Later.dmg` from
  older releases), verifies the app inside, swaps the bundle after quitting
  and relaunches. A copy that cannot replace itself opens the release page in
  the browser instead.

### Changed
- The app is now called Reader. The bundle is `Reader.app` and the release
  disk image is `Reader.dmg`. Updating from Read Later replaces
  `Read Later.app` with `Reader.app` in the same folder. The bundle
  identifier, library location, Keychain item and settings are unchanged, so
  existing libraries open as before.
- The repository is now `rakshithxaloori/macOS-reader`.

### Known issues
- The app is ad-hoc signed, so after each update macOS asks again before
  Reader can read its WhatsApp key from the Keychain.
- A Dock icon pinned to `Read Later.app` needs pinning again after the update
  turns it into `Reader.app`.
- An update briefly needs room for two copies of the app, which can take the
  app and library above 100 MB until the old copy is removed.

## [0.3.2] - 2026-11-03

### Added
- `scripts/dmg.sh` builds a downloadable `Read-Later.dmg` with a portable app
  that opens its own library in Application Support.
- `bundle.sh --portable` builds that shareable bundle.
- README sections on downloading the app and on forking it with a coding agent,
  and `docs/hacking.md`, a map of the code for agents.

### Changed
- `bundle.sh` points the app at `.local/native` only when that library exists,
  so a fresh clone or fork uses Application Support as documented.

## [0.3.1] - 2026-10-30

### Fixed
- "Database command could not be queued: sending on a full channel" no longer
  appears after the window was hidden or covered during feed checks. Any number
  of library changes now causes one reload, and a reload that finds the queue
  full is retried instead of being dropped, so the list cannot stay out of date.

## [0.3.0] - 2026-10-30

### Fixed
- RSS feeds hosted on WordPress.com (192.0.78.0/24) no longer fail with "URL
  resolves to a private network". Only 192.0.0.0/24 and 192.0.2.0/24 are
  treated as special-purpose.
- Full-text feeds no longer exhaust the storage budget on replay. Lean replay
  reserves space per entry rather than four times the XML size, so one large
  feed can no longer fill the shared replay buffer and block other feeds.
- Section headings with "header" classes (as on Substack) and horizontal rules
  inside wrappers are no longer dropped during extraction.
- The reader now draws horizontal rules. AppKit ignored the RTF border, so
  rules were invisible even when kept.
- Videos are no longer silently removed. Each one becomes a "Video · Watch on
  the original page" (or YouTube/Vimeo) link where the player was.
- Substack tweet embeds become a quoted post with the author, date and link
  instead of loose avatar, name and view-count lines.

### Changed
- Reader quotes are upright with an accent bar instead of indented italics,
  section headings (h3 and below) are smaller than top-level ones, and
  images get space before the next paragraph.
- Redesigned the Feeds screen. Feeds are grouped into Needs attention,
  Following and Paused cards with publication monograms, status dots,
  relative check times, a summary line, and a filter once there are six or
  more feeds. Errors show inline with Try again, and the More menu adds
  Copy feed address.
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
