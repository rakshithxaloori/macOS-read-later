# Read Later

Read Later is a read-later app and an RSS reader in one native Mac app. It is
written in Rust, runs entirely on your computer, and keeps everything it knows
in a single SQLite file.

## Why

I used Matter to save articles and follow feeds. It works well until you fall
behind. Matter only knows about the 20 or 30 newest items in a feed, so if a
publication posts more than that before you catch up, the older articles are
gone. You never see them, and you never find out you missed them.

That is backwards. The point of a reading list is that it waits for you.

So this app remembers every article it has ever seen in a feed. When a feed
slides past an article, the article stays in your Inbox until you read it,
archive it, or delete it. Replaying or shrinking a feed can add articles but
never removes them or resets what you have read.

The other thing I wanted was a fast way to save a link from my phone. Here you
send a message to yourself on WhatsApp, like `save https://example.com/post`,
and the article shows up in the Inbox on your Mac. No browser extension, no
share sheet, no account.

## What it does

- **Inbox and Archive.** Search, filter by publication or by where an article
  came from, sort by date, author, title, or reading time.
- **Feeds.** Subscribe to RSS and Atom, pause, sync, and unsubscribe. Feeds are
  checked every 15 minutes while the app is open.
- **WhatsApp saving.** Pair once with a QR code. Only messages you send to your
  own chat, containing one link and the word `save`, are read.
- **A real reader.** Full articles are fetched when you open them and rendered
  natively with headings, links, lists, tables, code, footnotes, and images.
  Georgia, Baskerville, Avenir Next, or Helvetica Neue; Light or Dark.
- **Import and export.** Matter CSV or ZIP, OPML, browser bookmarks, a CSV of
  URLs, and the app's own portable JSON or ZIP.
- **Magazines.** Turn a handful of saved articles into a printable PDF issue,
  with themes, columns, and paper sizes.

## How it is different

It is small. The app and all of its data stay under 100 MB. It does this by
keeping what matters forever (every article's title, link, preview, author, and
whether you have read it) and treating full article text as a cache. Bodies and
images live in a 20 MB cache for seven days and are fetched again when needed.
The tradeoff is that offline reading only works for articles you opened
recently, and if a publisher deletes an article, the full text can go with it.

It is local. There are no accounts, no servers, no telemetry, and no
subscription. There is no web view or JavaScript runtime either; the interface
is egui and the article view uses AppKit.

## Build and run

You need macOS and Rust. The pinned toolchain in `rust-toolchain.toml` is
installed automatically by rustup.

```sh
git clone https://github.com/rakshithxaloori/macOS-read-later.git
cd macOS-read-later
cargo run --release
```

To build a signed `.app` bundle in `.local/dist`:

```sh
./packaging/macos/bundle.sh
```

Then open `.local/dist/Read Later.app`, or drag it into Applications. Your
library is created in `~/Library/Application Support`. Pass `--data <dir>` to
use another directory, or `--offline` to open it without any network activity.

To run formatting, Clippy, and the tests:

```sh
./scripts/verify.sh
```

The tests use synthetic data. They never contact publishers or WhatsApp.

## More

- [User guide](docs/guide.md) covers each screen in detail.
- [Architecture](docs/architecture.md) explains how the pieces fit together.
- [Storage](docs/storage.md) explains the 100 MB budget and the cache.
- [CHANGELOG.md](CHANGELOG.md) lists what changed in each release.

## License

MIT. See [LICENSE](LICENSE).
