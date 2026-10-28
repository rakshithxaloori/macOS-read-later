use anyhow::Result;
use macos_read_later::{db_compat, domain::OWNER, whatsapp::*};
use rusqlite::params;

fn owner() -> VerifiedOwner {
    VerifiedOwner::verify(
        "12025550100",
        "12025550100:3@s.whatsapp.net",
        Some("12345@lid"),
        100,
    )
    .unwrap()
}
fn arrival(text: &str) -> Arrival<'_> {
    Arrival {
        chat: "12025550100@s.whatsapp.net",
        sender: "12025550100:2@s.whatsapp.net",
        alternate_chat: None,
        outgoing: true,
        group: false,
        historical: false,
        id: "NEW",
        timestamp: 101,
        text,
    }
}

#[test]
fn commands_require_verified_new_outgoing_self_chat() {
    let owner = owner();
    for text in [
        "save https://example.org/article#fragment",
        "HTTPS://EXAMPLE.ORG/article SAVE",
        "SaVe\nhttps://example.org/article",
    ] {
        assert_eq!(
            self_chat_command(&owner, &arrival(text), 102)
                .unwrap()
                .as_str(),
            "https://example.org/article"
        );
    }
    for text in [
        "https://example.org",
        "saved https://example.org",
        "savehttps://example.org",
        "save https://one.org https://two.org",
        "please save https://example.org",
        "save ftp://example.org",
        "save https:example.org",
        "save https://user:pass@example.org",
        "save http://127.0.0.1",
        "save http://localhost",
        "save http://[::1]",
        "save https://example.org:1234",
    ] {
        assert!(
            self_chat_command(&owner, &arrival(text), 102).is_none(),
            "{text}"
        );
    }
    let mut message = arrival("save https://example.org");
    message.historical = true;
    assert!(self_chat_command(&owner, &message, 102).is_none());
    message.historical = false;
    message.outgoing = false;
    assert!(self_chat_command(&owner, &message, 102).is_none());
    message.outgoing = true;
    message.group = true;
    assert!(self_chat_command(&owner, &message, 102).is_none());
    message.group = false;
    message.timestamp = 99;
    assert!(self_chat_command(&owner, &message, 102).is_none());
    message.timestamp = 163;
    assert!(self_chat_command(&owner, &message, 102).is_none());
    message.timestamp = 101;
    message.chat = "other@s.whatsapp.net";
    assert!(self_chat_command(&owner, &message, 102).is_none());
    message.chat = "12345@lid";
    message.alternate_chat = Some("12025550100@s.whatsapp.net");
    assert!(self_chat_command(&owner, &message, 102).is_some());
    message.alternate_chat = Some("other@s.whatsapp.net");
    assert!(self_chat_command(&owner, &message, 102).is_none());
    assert!(VerifiedOwner::verify("12025550100", "12025550101@s.whatsapp.net", None, 100).is_err());
}

#[test]
fn durable_arrival_receipt_replay_fences_and_read_state() -> Result<()> {
    let directory = tempfile::tempdir()?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let path = directory.path().join("library.sqlite3");
    let mut connection = db_compat::open(&path)?;
    db_compat::initialize(&connection)?;
    initialize(&connection)?;
    connection.execute(
        "INSERT INTO whatsapp(user_id,enabled,phone,status) VALUES(?1,1,'12025550100','connected')",
        [OWNER],
    )?;
    connection.execute("INSERT INTO native_whatsapp_control(user_id,epoch,session_id,expected_phone,activated_at) VALUES(?1,'epoch','native','12025550100',now())",[OWNER])?;
    let chat = "12025550100@s.whatsapp.net";
    let sender = "12025550100:2@s.whatsapp.net";
    assert!(commit_arrival(
        &mut connection,
        "epoch",
        chat,
        sender,
        "ID",
        "https://example.org/article"
    )?);
    let key = source_key(chat, sender, "ID");
    let (id, ack): (String, String) = connection.query_row(
        "SELECT article_id,ack_id FROM whatsapp_receipts WHERE user_id=?1 AND message_key=?2",
        params![OWNER, key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    connection.execute(
        "UPDATE articles SET read_at=now(),archived_at=now() WHERE id=?1",
        [&id],
    )?;
    drop(connection);
    let mut connection = db_compat::open(&path)?;
    assert!(!commit_arrival(
        &mut connection,
        "epoch",
        chat,
        sender,
        "ID",
        "https://example.org/article"
    )?);
    let replay_ack: String = connection.query_row(
        "SELECT ack_id FROM whatsapp_receipts WHERE user_id=?1 AND message_key=?2",
        params![OWNER, key],
        |row| row.get(0),
    )?;
    assert_eq!(ack, replay_ack);
    assert!(connection.query_row(
        "SELECT read_at IS NOT NULL AND archived_at IS NOT NULL FROM articles WHERE id=?1",
        [&id],
        |row| row.get::<_, bool>(0)
    )?);
    assert_eq!(
        connection.query_row("SELECT count(*) FROM native_whatsapp_arrivals", [], |row| {
            row.get::<_, i64>(0)
        })?,
        1
    );
    connection.execute(
        "UPDATE native_whatsapp_control SET epoch='new' WHERE user_id=?1",
        [OWNER],
    )?;
    assert!(
        commit_arrival(
            &mut connection,
            "epoch",
            chat,
            sender,
            "STALE",
            "https://example.org/second"
        )
        .is_err()
    );
    assert_eq!(
        connection.query_row("SELECT count(*) FROM articles", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn encrypted_native_keys_survive_restart_and_reject_other_owner() -> Result<()> {
    use whatsapp_rust::store::traits::SignalStore;
    let directory = tempfile::tempdir()?;
    macos_read_later::storage::Storage::new(directory.path(), 45_000_000)?;
    let path = directory.path().join("session.sqlite3");
    let master = [42; 32];
    let identity = [77; 32];
    let store = macos_read_later::credentials::encrypted_store(&path, &master, OWNER).await?;
    store
        .put_identity("synthetic@s.whatsapp.net", identity)
        .await?;
    drop(store);
    let store = macos_read_later::credentials::encrypted_store(&path, &master, OWNER).await?;
    assert_eq!(
        store.load_identity("synthetic@s.whatsapp.net").await?,
        Some(identity)
    );
    let bytes = std::fs::read(&path)?;
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(!bytes.windows(32).any(|window| window == identity));
    let wrong = rusqlite::Connection::open(&path)?;
    let key = macos_read_later::credentials::derive_owner_key(&master, "different-owner");
    wrong.execute_batch(&format!(
        "PRAGMA key = \"x'{}'\"",
        hex::encode(key.as_ref())
    ))?;
    assert!(
        wrong
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| row
                .get::<_, i64>(0))
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn credential_quota_refusal_does_not_create_a_session_file() -> Result<()> {
    let directory = tempfile::tempdir()?;
    macos_read_later::storage::Storage::new(directory.path(), 96_000_000)?;
    let path = directory.path().join("whatsapp/session.sqlite3");
    let result = macos_read_later::credentials::encrypted_store(&path, &[31; 32], OWNER).await;
    assert!(result.is_err());
    assert!(!path.exists());
    Ok(())
}
