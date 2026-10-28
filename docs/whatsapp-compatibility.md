# Rust WhatsApp compatibility

The offline proof passes with the published `whatsapp-rust =0.7.0` and
`whatsapp-rust-sqlite-storage =0.7.0` crates. The native implementation lives in
`src/whatsapp.rs` and `src/credentials.rs`. Pairing, network reconnection and live
message delivery require scanning the app's QR from your phone. No phone is
paired and no message is sent by these checks.

## Connector and pinned API

The connector performs the WebSocket/Noise/Signal protocol in Rust; it does not
start a browser, Node or Baileys. Its builder exposes rotating QR payloads with
validity durations, authenticated phone/LID identity, history-sync suppression,
custom durable storage and inbound durability hooks. The released send API
accepts a caller-supplied message ID for retryable acknowledgements. See the
[published connector source](https://docs.rs/crate/whatsapp-rust/0.7.0/source/src/bot.rs)
and [released send implementation](https://docs.rs/crate/whatsapp-rust/0.7.0/source/src/send/mod.rs).

Published 0.7.0 differs materially from the upstream branch at
`edaa64b68d0de24bdb6848b98a824a32e276c55c`. The release uses
`SqliteStoreConfig`, `SqliteStore::with_config` and `libsqlite3-sys 0.37`.
Upstream uses renamed database APIs and `libsqlite3-sys 0.38`. Use the checked
release and lockfiles. The high-level `enable_auto_reconnect` field is public in
the release; its current upstream accessor APIs must not be copied into this
version.

## Encrypted native credentials

Only fresh native sessions are opened. A 32-byte master is generated with
macOS `SecRandom`, saved as a native Keychain generic password under this app's
service identity, and derived with HKDF-SHA256 and the storage owner's UUID.
The resulting key opens an independent SQLCipher database. Its WAL and all
protocol tables are encrypted, including device keys and pending inbound
buffers. Plain Read Later content remains an ordinary unkeyed SQLite database.
No environment file, external key or Baileys credential is read.

The released storage crate supplies a per-connection hook before its own
pragmas/migrations. The adapter keys every pooled connection, verifies
`cipher_version` and a schema read, and selects FULL synchronization plus a
10-second busy timeout. A missing SQLCipher build fails closed. See the
[released storage configuration](https://docs.rs/crate/whatsapp-rust-sqlite-storage/0.7.0/source/src/sqlite_store.rs),
[SQLCipher support in libsqlite3-sys](https://github.com/rusqlite/rusqlite/tree/master/libsqlite3-sys),
and [macOS protected password APIs](https://docs.rs/security-framework/3.7.0/security_framework/passwords/index.html).

Private directories are mode 0700, session files mode 0600, and symlink session
paths are rejected. Each pairing uses its own UUID file name. Disconnect fences
old callbacks, stops transport, erases this native session and its Keychain
master, and retains library arrivals/receipts. Quit stops transport while retaining
native credentials for explicit reconnect/restart. Window visibility does not own
this actor.

## Intake and receipt durability

The native filter requires the authenticated phone to match the expected number,
then accepts only outgoing direct self-chat messages with an ID and timestamp
at/after activation and at most 60 seconds into the future. It recognizes the
verified PN/LID aliases, rejects conflicting alternate identities, and requires
exactly two whitespace-separated tokens: standalone case-insensitive `save` and
one HTTP(S) URL. Bare links, other chats, history, groups, invalid URLs and
multiple-link commands are ignored. SDK history-sync download is disabled.
Offline delivery of a new post-activation message remains eligible.

PN/LID aliases for self-chat are canonicalized before durable deduplication.
A source key hashes length-delimited canonical chat/sender/message ID fields.
The SDK's inbound durability hook invokes the article/provenance/arrival/receipt
transaction before it permits a transport acknowledgement. Replays preserve the
same receipt ID and reading/archive/deletion state. A random session epoch and
the enabled state fence stale callbacks and completion writes.

The article acknowledgement is sent separately after commit. It uses the same
stored message ID on retry and marks `sent_at` only after success. It never sends
inside the inbound Signal lock. See the
[released durability-hook contract](https://docs.rs/crate/whatsapp-rust/0.7.0/source/src/types/durability_hook.rs).
The upstream contract identifies a residual edge case: a failed pending-buffer
write can advance in-memory Signal state before redelivery. Disk-full recovery
therefore remains a live fault-injection check, not an exactly-once claim.

## Reproduce the offline proof

From this repository:

```sh
CARGO_TARGET_DIR=.local/targets/whatsapp scripts/cargo-local.sh test \
  --locked --manifest-path proofs/whatsapp/Cargo.toml
```

The three proof tests cover:

- Strict new self-chat command/identity/timestamp rejection and PN/LID checks.
- Durable synthetic arrival replay and unchanged acknowledgement ID after restart.
- Encrypted identities survive restart; wrong owner and unkeyed reads fail;
  ciphertext lacks a SQLite header and the synthetic plaintext key. Bot setup
  compiles without starting it. An unkeyed SQLite database under the same linkage
  still enforces foreign keys and an immutable trigger.

`tests/whatsapp.rs` exercises the native implementation against the actual
retained library schema, including atomic article/receipt replay, stale-epoch
rejection and preserved reading state. Root project verification reports its
results separately.

These checks use disposable synthetic databases and synthetic phone numbers.
Keychain reads/writes, QR generation from the service, phone scanning, actual
outgoing self-chat delivery, uncertain acknowledgement-send recovery and
reconnection after sleep/network loss remain unrun. The UI must say disconnected
until fresh pairing succeeds; this proof is not evidence of a paired phone.
