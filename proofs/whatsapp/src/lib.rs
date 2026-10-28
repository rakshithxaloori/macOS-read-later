//! Offline compatibility proof. Nothing here starts a network session.
use anyhow::{Result, bail};
use std::time::Duration;
use whatsapp_rust_sqlite_storage::{SqliteStore, SqliteStoreConfig, Synchronous};
use zeroize::Zeroizing;

pub fn owner_key(master: &[u8; 32], owner: &str) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0; 32]);
    hkdf::Hkdf::<sha2::Sha256>::new(Some(b"Reading List native WhatsApp v1"), master)
        .expand(owner.as_bytes(), key.as_mut())
        .expect("fixed 32-byte HKDF output is valid");
    key
}

/// All pooled connections are keyed before migrations/WAL; refuse ordinary SQLite.
pub async fn encrypted_store(path: &str, master: &[u8; 32], owner: &str) -> Result<SqliteStore> {
    use diesel::prelude::*;
    #[derive(QueryableByName)]
    struct CipherVersion {
        #[diesel(sql_type = diesel::sql_types::Text)]
        cipher_version: String,
    }
    let key = owner_key(master, owner);
    let config = SqliteStoreConfig {
        busy_timeout: Duration::from_secs(10),
        synchronous: Synchronous::Full,
        ..Default::default()
    }
    .with_connection_init(move |connection| {
        // hex only: secrets cannot introduce SQL syntax and errors never contain the key.
        diesel::sql_query(format!("PRAGMA key = \"x'{}'\"", hex::encode(key.as_ref())))
            .execute(connection)?;
        let version =
            diesel::sql_query("PRAGMA cipher_version").get_result::<CipherVersion>(connection)?;
        if version.cipher_version.is_empty() {
            return Err("SQLCipher is unavailable".into());
        }
        diesel::sql_query("SELECT count(*) FROM sqlite_master").execute(connection)?;
        Ok(())
    });
    Ok(SqliteStore::with_config(path, config).await?)
}

/// This compile-checks macOS protected storage; tests never touch the real Keychain.
#[cfg(target_os = "macos")]
pub fn keychain_master(owner: &str) -> Result<Zeroizing<[u8; 32]>> {
    let bytes = security_framework::passwords::get_generic_password(
        "org.reading-list.native.whatsapp.v1",
        owner,
    )?;
    let master: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid native key"))?;
    Ok(Zeroizing::new(master))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedOwner {
    pub phone_jid: String,
    pub lid: Option<String>,
    pub activated_at: i64,
}

impl VerifiedOwner {
    /// Verify the server-authenticated PN before accepting any messages.
    pub fn verify(
        expected_phone: &str,
        authenticated_pn: &str,
        lid: Option<&str>,
        activated_at: i64,
    ) -> Result<Self> {
        if !(8..=15).contains(&expected_phone.len())
            || !expected_phone.bytes().all(|c| c.is_ascii_digit())
            || expected_phone.starts_with('0')
        {
            bail!("invalid expected phone");
        }
        let phone_jid = format!("{expected_phone}@s.whatsapp.net");
        if bare_jid(authenticated_pn) != phone_jid {
            bail!("paired account mismatch");
        }
        Ok(Self {
            phone_jid,
            lid: lid.map(bare_jid),
            activated_at,
        })
    }
}

pub fn bare_jid(value: &str) -> String {
    let Some((user, server)) = value.split_once('@') else {
        return value.to_string();
    };
    let user = match user.rsplit_once(':') {
        Some((bare, device))
            if !device.is_empty() && device.bytes().all(|c| c.is_ascii_digit()) =>
        {
            bare
        }
        _ => user,
    };
    format!("{user}@{server}")
}

pub struct Arrival<'a> {
    pub chat: &'a str,
    pub alternate_chat: Option<&'a str>,
    pub sender: &'a str,
    pub outgoing: bool,
    pub group: bool,
    pub historical: bool,
    pub id: &'a str,
    pub timestamp: i64,
    pub text: &'a str,
}

pub fn command(owner: &VerifiedOwner, arrival: &Arrival<'_>, now: i64) -> Option<String> {
    let chat = bare_jid(arrival.chat);
    let own = |jid: &str| jid == owner.phone_jid || owner.lid.as_deref() == Some(jid);
    if !arrival.outgoing
        || arrival.group
        || arrival.historical
        || arrival.id.is_empty()
        || !own(&chat)
        || !own(&bare_jid(arrival.sender))
        || arrival.timestamp < owner.activated_at
        || arrival.timestamp > now.saturating_add(60)
    {
        return None;
    }
    if let Some(alt) = arrival.alternate_chat {
        let expected = if chat == owner.phone_jid {
            owner.lid.as_deref()
        } else {
            Some(owner.phone_jid.as_str())
        };
        if expected != Some(bare_jid(alt).as_str()) {
            return None;
        }
    }
    let parts: Vec<_> = arrival.text.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let candidate = if parts[0].eq_ignore_ascii_case("save") {
        parts[1]
    } else if parts[1].eq_ignore_ascii_case("save") {
        parts[0]
    } else {
        return None;
    };
    if !(candidate
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || candidate
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://")))
    {
        return None;
    }
    if candidate.len() > 4096 {
        return None;
    }
    let mut url = url::Url::parse(candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| !matches!(port, 80 | 443))
    {
        return None;
    }
    let host = url.host_str()?;
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return None;
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        let public = match ip {
            std::net::IpAddr::V4(v) => {
                !v.is_private()
                    && !v.is_loopback()
                    && !v.is_link_local()
                    && !v.is_unspecified()
                    && !v.is_multicast()
                    && !v.is_broadcast()
            }
            std::net::IpAddr::V6(v) => {
                !v.is_loopback()
                    && !v.is_unspecified()
                    && !v.is_unique_local()
                    && !v.is_unicast_link_local()
                    && !v.is_multicast()
            }
        };
        if !public {
            return None;
        }
    }
    url.set_fragment(None);
    Some(url.into())
}

/// Durable arrival and retryable ack ID are committed together before SDK transport ack.
pub fn persist_arrival(
    connection: &mut rusqlite::Connection,
    owner: &str,
    chat: &str,
    sender: &str,
    id: &str,
    url: &str,
    ack_id: &str,
) -> Result<bool> {
    let transaction = connection.transaction()?;
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO arrivals(owner,chat,sender,message_id,url,ack_id) VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![owner, chat, sender, id, url, ack_id],
    )?;
    transaction.commit()?;
    Ok(inserted == 1)
}

pub const ARRIVAL_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS arrivals(owner TEXT NOT NULL,chat TEXT NOT NULL,sender TEXT NOT NULL,message_id TEXT NOT NULL,url TEXT NOT NULL,ack_id TEXT NOT NULL,sent_at TEXT,PRIMARY KEY(owner,chat,sender,message_id));";

/// Compile the connector's exact published pairing, owner, reconnect and receipt APIs.
/// This configures callbacks but never calls run/spawn or sends messages.
pub async fn configure_connector(store: SqliteStore) -> Result<whatsapp_rust::bot::Bot> {
    use whatsapp_rust::prelude::*;
    let bot = Bot::builder()
        .with_backend(store)
        .skip_history_sync()
        .on_qr_code(|_code, _expiry| async {})
        .on_connected(|client| async move {
            let _ = (client.pn(), client.lid());
        })
        .on_message(|context| async move {
            let _ = (
                context.info.source.is_from_me,
                context.info.source.chat,
                context.info.timestamp,
            );
        })
        .build()
        .await?;
    bot.client()
        .enable_auto_reconnect
        .store(true, std::sync::atomic::Ordering::Release);
    Ok(bot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use whatsapp_rust::store::traits::SignalStore;

    fn owner() -> VerifiedOwner {
        VerifiedOwner::verify(
            "12025550100",
            "12025550100:7@s.whatsapp.net",
            Some("12345@lid"),
            100,
        )
        .unwrap()
    }
    fn arrival(text: &str) -> Arrival<'_> {
        Arrival {
            chat: "12025550100@s.whatsapp.net",
            alternate_chat: None,
            sender: "12025550100:2@s.whatsapp.net",
            outgoing: true,
            group: false,
            historical: false,
            id: "NEW-1",
            timestamp: 101,
            text,
        }
    }

    #[test]
    fn strict_new_self_chat_commands() {
        let owner = owner();
        for text in [
            "save https://example.org/article#part",
            "HTTPS://EXAMPLE.ORG/article SAVE",
            "SaVe\nhttps://example.org/article",
        ] {
            assert_eq!(
                command(&owner, &arrival(text), 102).as_deref(),
                Some("https://example.org/article")
            );
        }
        for text in [
            "https://example.org",
            "saved https://example.org",
            "savehttps://example.org",
            "save https://one.org https://two.org",
            "please save https://example.org",
            "save ftp://example.org",
            "save https://user:pass@example.org",
            "save http://127.0.0.1",
            "save http://localhost",
            "save http://[::1]",
            "save https://example.org:1234",
        ] {
            assert!(command(&owner, &arrival(text), 102).is_none(), "{text}");
        }
        let mut msg = arrival("save https://example.org");
        msg.historical = true;
        assert!(command(&owner, &msg, 102).is_none());
        msg.historical = false;
        msg.outgoing = false;
        assert!(command(&owner, &msg, 102).is_none());
        msg.outgoing = true;
        msg.group = true;
        assert!(command(&owner, &msg, 102).is_none());
        msg.group = false;
        msg.timestamp = 99;
        assert!(command(&owner, &msg, 102).is_none());
        msg.timestamp = 163;
        assert!(command(&owner, &msg, 102).is_none());
        msg.timestamp = 101;
        msg.chat = "other@s.whatsapp.net";
        assert!(command(&owner, &msg, 102).is_none());
        msg.chat = "12345@lid";
        msg.alternate_chat = Some("12025550100@s.whatsapp.net");
        assert!(command(&owner, &msg, 102).is_some());
        msg.alternate_chat = Some("other@s.whatsapp.net");
        assert!(command(&owner, &msg, 102).is_none());
        assert!(
            VerifiedOwner::verify("12025550100", "12025550101@s.whatsapp.net", None, 100).is_err()
        );
    }

    #[test]
    fn durable_arrivals_replay_and_ack_retry() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("arrivals.db");
        let mut db = Connection::open(&path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        db.execute_batch(ARRIVAL_SCHEMA)?;
        assert!(persist_arrival(
            &mut db,
            "owner",
            "self",
            "self",
            "ID",
            "https://example.org/",
            "ACK-1"
        )?);
        drop(db);
        let mut db = Connection::open(&path)?;
        assert!(!persist_arrival(
            &mut db,
            "owner",
            "self",
            "self",
            "ID",
            "https://example.org/",
            "ACK-CHANGED"
        )?);
        let ack: String = db.query_row(
            "SELECT ack_id FROM arrivals WHERE sent_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(ack, "ACK-1");
        db.execute("UPDATE arrivals SET sent_at='sent' WHERE owner='owner' AND message_id='ID' AND sent_at IS NULL", [])?;
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM arrivals WHERE sent_at IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        assert!(persist_arrival(
            &mut db,
            "owner",
            "other",
            "other",
            "ID",
            "https://example.org/",
            "ACK-2"
        )?);
        Ok(())
    }

    #[tokio::test]
    async fn sqlcipher_credentials_restart_owner_binding_and_connector_api() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("native-session.db");
        let master = [42; 32];
        let secret = [77; 32];
        let store = encrypted_store(path.to_str().unwrap(), &master, "owner-a").await?;
        store
            .put_identity("synthetic@s.whatsapp.net", secret)
            .await?;
        let bot = configure_connector(store.clone()).await?;
        assert!(
            bot.client()
                .enable_auto_reconnect
                .load(std::sync::atomic::Ordering::Acquire)
        );
        drop(bot);
        drop(store);
        let store = encrypted_store(path.to_str().unwrap(), &master, "owner-a").await?;
        assert_eq!(
            store.load_identity("synthetic@s.whatsapp.net").await?,
            Some(secret)
        );
        let disk = std::fs::read(&path)?;
        assert!(!disk.starts_with(b"SQLite format 3"));
        assert!(!disk.windows(32).any(|window| window == secret));
        let wrong = Connection::open(&path)?;
        wrong.execute_batch(&format!(
            "PRAGMA key = \"x'{}'\"",
            hex::encode(owner_key(&master, "owner-b").as_ref())
        ))?;
        assert!(
            wrong
                .query_row("SELECT count(*) FROM sqlite_master", [], |r| r
                    .get::<_, i64>(0))
                .is_err()
        );
        assert!(
            Connection::open(&path)?
                .query_row("SELECT count(*) FROM sqlite_master", [], |r| r
                    .get::<_, i64>(0))
                .is_err()
        );
        // SQLCipher's presence does not key ordinary databases implicitly.
        let plain = Connection::open(dir.path().join("plain-library.db"))?;
        plain.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE articles(id TEXT PRIMARY KEY, archived INTEGER NOT NULL); CREATE TABLE snapshots(article_id TEXT REFERENCES articles(id), text TEXT); CREATE TRIGGER immutable BEFORE UPDATE ON snapshots BEGIN SELECT RAISE(ABORT,'immutable'); END; INSERT INTO articles VALUES('a',0); INSERT INTO snapshots VALUES('a','complete');")?;
        assert!(
            plain
                .execute("UPDATE snapshots SET text='changed'", [])
                .is_err()
        );
        assert!(
            plain
                .execute("INSERT INTO snapshots VALUES('missing','body')", [])
                .is_err()
        );
        Ok(())
    }
}
