# Reader

Reader is a read-later app and an RSS reader in one native Mac app. It is
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

## Download

Get the latest version at [reader.rakshithxaloori.com](https://reader.rakshithxaloori.com)
or from [Releases](https://github.com/rakshithxaloori/macOS-reader/releases/latest).
Open `Reader.dmg` and drag Reader into Applications. It needs macOS 13
or later on an Apple silicon Mac.

The app is not notarized by Apple yet, so the first time you open it macOS says
it cannot check it for malicious software. Click **Done**, open **System
Settings → Privacy & Security**, scroll down, and click **Open Anyway** next to
Reader. You only need to do this once.

## Make it your own

Reader is meant to be forked. You can change anything about it by asking a
coding agent such as [Claude Code](https://claude.com/claude-code) or
[Codex](https://openai.com/codex), then build the result and use it as your
own app. You do not need to know Rust.

**1. Install the tools.** You need Xcode's command line tools, Rust, and git.

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**2. Fork and clone.** Click **Fork** at the top of this page, then clone your
copy (replace `YOUR-NAME`):

```sh
git clone https://github.com/YOUR-NAME/macOS-reader.git
cd macOS-reader
cargo run --release
```

The first build takes a few minutes. When the window opens, the app works.

**3. Ask your agent for a change.** Start your agent in the folder (`claude`
for Claude Code, `codex` for Codex) and describe what you want, for example:

> Read docs/hacking.md first. Then add a "Read time" sort option to the Inbox
> and run ./scripts/verify.sh when you are done.

[docs/hacking.md](docs/hacking.md) tells the agent where things are, how to
check its work, and the rules that keep the app small and your data safe.

**4. Build your app.** When you like the change:

```sh
./packaging/macos/bundle.sh
```

Open `.local/dist/Reader.app`, or drag it into Applications to replace the
downloaded version. Your library lives in `~/Library/Application Support`, so it
carries over between builds. Pass `--data <dir>` to use another directory, or
`--offline` to open the app without any network activity. To share your build
with someone else, `./scripts/dmg.sh` makes a disk image in `.local/releases`.

**5. Keep up with this repository.** To pull in new releases while keeping your
changes:

```sh
git remote add upstream https://github.com/rakshithxaloori/macOS-reader.git
git pull upstream main
```

If there is a conflict, ask your agent to resolve it and run
`./scripts/verify.sh` again.

To run formatting, Clippy, and the tests yourself:

```sh
./scripts/verify.sh
```

The tests use synthetic data. They never contact publishers or WhatsApp.

## More

- [User guide](docs/guide.md) covers each screen in detail.
- [Changing the app](docs/hacking.md) is the map for you and your coding agent.
- [Architecture](docs/architecture.md) explains how the pieces fit together.
- [Storage](docs/storage.md) explains the 100 MB budget and the cache.
- [CHANGELOG.md](CHANGELOG.md) lists what changed in each release.

## License

MIT. See [LICENSE](LICENSE).
