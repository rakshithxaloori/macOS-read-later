//! Rust WhatsApp transport, fenced native session state and durable self-chat intake.
use crate::{
    credentials,
    domain::{ArrivalKind, OWNER},
};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
use whatsapp_rust::{InboundDurabilityHook, prelude::*};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Pairing,
    Connected,
    Disconnecting,
    Failed,
}
impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Pairing => "Pairing",
            Self::Connected => "Connected",
            Self::Disconnecting => "Disconnecting",
            Self::Failed => "Failed",
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Pairing => "pairing",
            Self::Connected => "connected",
            Self::Disconnecting => "disconnecting",
            Self::Failed => "failed",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct WhatsAppView {
    pub state: ConnectionState,
    pub phone: Option<String>,
    /// QR payload, held only in memory. Never log it or export it.
    pub qr: Option<String>,
    pub qr_expires_at: Option<i64>,
    pub error: Option<String>,
}
#[derive(Debug)]
pub enum WhatsAppCommand {
    Connect { phone: String },
    Disconnect,
    StartReconnect,
    Quit,
}
#[derive(Clone, Debug)]
pub enum WhatsAppEvent {
    State(WhatsAppView),
    LibraryChanged,
}
#[derive(Clone)]
pub struct WhatsAppConfig {
    pub library_path: PathBuf,
    pub private_directory: PathBuf,
    pub offline: bool,
}

pub struct WhatsAppHandle {
    pub commands: mpsc::Sender<WhatsAppCommand>,
    task: tokio::task::JoinHandle<()>,
}
impl WhatsAppHandle {
    pub async fn shutdown(self) {
        let _ = self.commands.send(WhatsAppCommand::Quit).await;
        let _ = self.task.await;
    }
}

/// Call inside the app's Tokio runtime. Starting this actor never enables an imported session.
pub fn spawn(
    config: WhatsAppConfig,
    events: mpsc::Sender<WhatsAppEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> WhatsAppHandle {
    let (commands, receiver) = mpsc::channel(32);
    let task = tokio::spawn(async move {
        let mut worker = Worker {
            config,
            events,
            wake,
            session: None,
            view: WhatsAppView::default(),
            changed: Arc::new(Notify::new()),
        };
        if let Err(error) = worker.run(receiver).await {
            worker.view.state = ConnectionState::Failed;
            worker.view.error = Some(format!("WhatsApp stopped: {error}"));
            worker.publish();
        }
    });
    WhatsAppHandle { commands, task }
}

pub const NATIVE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS native_whatsapp_control(user_id TEXT PRIMARY KEY REFERENCES users(id), epoch TEXT NOT NULL, session_id TEXT, expected_phone TEXT, activated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_whatsapp_arrivals(user_id TEXT NOT NULL,chat TEXT NOT NULL,sender TEXT NOT NULL,message_id TEXT NOT NULL,source_key TEXT NOT NULL UNIQUE,received_at TEXT NOT NULL DEFAULT(now()),PRIMARY KEY(user_id,chat,sender,message_id),FOREIGN KEY(user_id) REFERENCES users(id));";

pub fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(NATIVE_SCHEMA)?;
    Ok(())
}

fn activation_second(value: &str) -> Result<i64> {
    let date = chrono::DateTime::parse_from_rfc3339(value)?;
    Ok(date.timestamp() + i64::from(date.timestamp_subsec_nanos() != 0))
}

pub fn validate_phone(phone: &str) -> Result<()> {
    ensure!(
        (8..=15).contains(&phone.len())
            && !phone.starts_with('0')
            && phone.bytes().all(|byte| byte.is_ascii_digit()),
        "Enter your country code and phone number using 8–15 digits"
    );
    Ok(())
}
pub fn bare_jid(value: &str) -> String {
    let Some((user, server)) = value.split_once('@') else {
        return value.into();
    };
    let user = match user.rsplit_once(':') {
        Some((bare, device))
            if !device.is_empty() && device.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            bare
        }
        _ => user,
    };
    format!("{user}@{server}")
}
#[derive(Clone, Debug)]
pub struct VerifiedOwner {
    pub phone_jid: String,
    pub lid: Option<String>,
    pub activated_at: i64,
}
impl VerifiedOwner {
    pub fn verify(
        phone: &str,
        authenticated_pn: &str,
        lid: Option<&str>,
        activated_at: i64,
    ) -> Result<Self> {
        validate_phone(phone)?;
        let phone_jid = format!("{phone}@s.whatsapp.net");
        ensure!(
            bare_jid(authenticated_pn) == phone_jid,
            "The scanned account does not match your number"
        );
        Ok(Self {
            phone_jid,
            lid: lid.map(bare_jid),
            activated_at,
        })
    }
}
pub struct Arrival<'a> {
    pub chat: &'a str,
    pub sender: &'a str,
    pub alternate_chat: Option<&'a str>,
    pub outgoing: bool,
    pub group: bool,
    pub historical: bool,
    pub id: &'a str,
    pub timestamp: i64,
    pub text: &'a str,
}
pub fn self_chat_command(
    owner: &VerifiedOwner,
    arrival: &Arrival<'_>,
    now: i64,
) -> Option<url::Url> {
    let own = |jid: &str| jid == owner.phone_jid || owner.lid.as_deref() == Some(jid);
    let chat = bare_jid(arrival.chat);
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
    let input = if parts[0].eq_ignore_ascii_case("save") {
        parts[1]
    } else if parts[1].eq_ignore_ascii_case("save") {
        parts[0]
    } else {
        return None;
    };
    if !(input
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || input
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://")))
    {
        return None;
    }
    crate::network::normalize_url(input).ok()
}

/// Composite source IDs prevent collisions between chats and remain stable on replay.
pub fn source_key(chat: &str, sender: &str, message_id: &str) -> String {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for value in [bare_jid(chat), bare_jid(sender), message_id.into()] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    format!("native:{}", hex::encode(digest.finalize()))
}

/// Article/provenance/receipt and composite arrival commit in the same transaction.
pub fn commit_arrival(
    connection: &mut Connection,
    epoch: &str,
    chat: &str,
    sender: &str,
    message_id: &str,
    url: &str,
) -> Result<bool> {
    let _reservation = if crate::db_compat::is_lean(connection)?
        && crate::storage::database_path(connection)?.is_some()
    {
        Some(
            crate::storage::Storage::for_database(connection)?
                .reserve("WhatsApp arrival", 262144)?,
        )
    } else {
        None
    };
    let transaction = connection.transaction()?;
    let valid:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM native_whatsapp_control n JOIN whatsapp w ON w.user_id=n.user_id WHERE n.user_id=?1 AND n.epoch=?2 AND w.enabled=1)",params![OWNER,epoch],|row|row.get(0))?;
    ensure!(valid, "WhatsApp session is no longer active");
    let key = source_key(chat, sender, message_id);
    let inserted=transaction.execute("INSERT OR IGNORE INTO native_whatsapp_arrivals(user_id,chat,sender,message_id,source_key) VALUES(?1,?2,?3,?4,?5)",params![OWNER,bare_jid(chat),bare_jid(sender),message_id,key])?>0;
    if inserted {
        let (article, created) = crate::library::save_in_transaction(
            &transaction,
            url,
            ArrivalKind::WhatsApp,
            &key,
            "WhatsApp",
            None,
            None,
        )?;
        let ack = uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .to_ascii_uppercase();
        transaction.execute("INSERT INTO whatsapp_receipts(user_id,message_key,article_id,created_article,ack_id) VALUES(?1,?2,?3,?4,?5)",params![OWNER,key,article.0,created,ack])?;
    }
    transaction.commit()?;
    Ok(inserted)
}

struct IntakeHook {
    config: WhatsAppConfig,
    epoch: String,
    phone: String,
    activated_at: i64,
    active: Arc<AtomicBool>,
    changed: Arc<Notify>,
}
#[async_trait::async_trait]
impl InboundDurabilityHook for IntakeHook {
    async fn on_messages(&self, client: Arc<Client>, batch: &[InboundMessage]) -> Result<()> {
        ensure!(
            self.active.load(Ordering::Acquire),
            "WhatsApp session stopped"
        );
        let pn = client
            .pn()
            .context("Paired WhatsApp account is not verified")?
            .to_string();
        let lid = client.lid().map(|jid| jid.to_string());
        let owner = VerifiedOwner::verify(&self.phone, &pn, lid.as_deref(), self.activated_at)?;
        let now = chrono::Utc::now().timestamp();
        let mut arrivals = Vec::new();
        for message in batch {
            let info = &message.info;
            let source = &info.source;
            let chat = source.chat.to_string();
            let sender = source.sender.to_string();
            let alternate = source.recipient_alt.as_ref().map(|jid| jid.to_string());
            let arrival = Arrival {
                chat: &chat,
                sender: &sender,
                alternate_chat: alternate.as_deref(),
                outgoing: source.is_from_me,
                group: source.is_group,
                historical: false,
                id: info.id.as_str(),
                timestamp: info.timestamp.timestamp(),
                text: message.message.text_content().unwrap_or_default(),
            };
            if let Some(url) = self_chat_command(&owner, &arrival, now) {
                arrivals.push((
                    owner.phone_jid.clone(),
                    owner.phone_jid.clone(),
                    info.id.to_string(),
                    url.to_string(),
                ));
            }
        }
        if arrivals.is_empty() {
            return Ok(());
        }
        let path = self.config.library_path.clone();
        let epoch = self.epoch.clone();
        let active = self.active.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut connection = crate::db_compat::open(&path)?;
            for (chat, sender, id, url) in arrivals {
                ensure!(active.load(Ordering::Acquire), "WhatsApp session stopped");
                commit_arrival(&mut connection, &epoch, &chat, &sender, &id, &url)?;
            }
            Ok(())
        })
        .await??;
        self.changed.notify_one();
        Ok(())
    }
}

#[derive(Debug)]
enum Notice {
    Qr {
        epoch: String,
        code: String,
        expiry: Duration,
    },
    Connected {
        epoch: String,
        pn: Option<String>,
        lid: Option<String>,
    },
    Disconnected {
        epoch: String,
    },
    LoggedOut {
        epoch: String,
    },
}
struct Session {
    epoch: String,
    path: PathBuf,
    phone: String,
    activated_at: i64,
    active: Arc<AtomicBool>,
    handle: BotHandle,
    notices: mpsc::Receiver<Notice>,
}
struct Worker {
    config: WhatsAppConfig,
    events: mpsc::Sender<WhatsAppEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    session: Option<Session>,
    view: WhatsAppView,
    changed: Arc<Notify>,
}
impl Worker {
    fn publish(&self) {
        let _ = self
            .events
            .try_send(WhatsAppEvent::State(self.view.clone()));
        (self.wake)();
    }
    fn library_changed(&self) {
        let _ = self.events.try_send(WhatsAppEvent::LibraryChanged);
        (self.wake)();
    }
    async fn db<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let path = self.config.library_path.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = crate::db_compat::open(&path)?;
            let _reservation = if crate::db_compat::is_lean(&connection)? {
                Some(
                    crate::storage::Storage::for_database(&connection)?
                        .reserve("WhatsApp state", 262144)?,
                )
            } else {
                None
            };
            operation(&mut connection)
        })
        .await?
    }
    async fn run(&mut self, mut commands: mpsc::Receiver<WhatsAppCommand>) -> Result<()> {
        self.db(|c| initialize(c)).await?;
        self.publish();
        // Only the native control row can resume a native session; source credentials are irrelevant.
        if !self.config.offline
            && let Err(error) = self.reconnect().await
        {
            self.view.state = ConnectionState::Failed;
            self.view.error = Some(error.to_string());
            self.publish();
        }
        loop {
            enum Input {
                Command(Option<WhatsAppCommand>),
                Notice(Option<Notice>),
                Changed,
                Retry,
            }
            let input = if let Some(session) = self.session.as_mut() {
                tokio::select! {
                    command=commands.recv()=>Input::Command(command),notice=session.notices.recv()=>Input::Notice(notice),
                    ()=self.changed.notified()=>Input::Changed,
                    ()=tokio::time::sleep(Duration::from_secs(15))=>Input::Retry,
                }
            } else {
                Input::Command(commands.recv().await)
            };
            let result = match input {
                Input::Command(Some(WhatsAppCommand::Connect { phone })) => {
                    self.connect(phone).await
                }
                Input::Command(Some(WhatsAppCommand::Disconnect)) => self.disconnect(true).await,
                Input::Command(Some(WhatsAppCommand::StartReconnect)) => self.reconnect().await,
                Input::Command(Some(WhatsAppCommand::Quit) | None) => {
                    self.disconnect(false).await?;
                    return Ok(());
                }
                Input::Notice(Some(notice)) => self.notice(notice).await,
                Input::Notice(None) => {
                    self.disconnect(false).await?;
                    bail!("WhatsApp transport stopped")
                }
                Input::Changed => {
                    self.library_changed();
                    self.send_receipts().await
                }
                Input::Retry => {
                    if self
                        .view
                        .qr_expires_at
                        .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp())
                    {
                        self.view.qr = None;
                        self.view.qr_expires_at = None;
                        self.publish();
                    }
                    self.send_receipts().await
                }
            };
            if let Err(error) = result {
                self.view.error = Some(error.to_string());
                self.publish();
            }
        }
    }
    async fn connect(&mut self, phone: String) -> Result<()> {
        ensure!(!self.config.offline, "WhatsApp is disabled in offline mode");
        validate_phone(&phone)?;
        ensure!(
            self.session.is_none(),
            "Disconnect the current session before pairing again"
        );
        let enabled = self
            .db(|c| {
                Ok(c.query_row(
                    "SELECT enabled FROM whatsapp WHERE user_id=?1",
                    [OWNER],
                    |r| r.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
            })
            .await?;
        ensure!(
            !enabled,
            "Disconnect the current session before pairing again"
        );
        let epoch = uuid::Uuid::new_v4().to_string();
        let session_id = uuid::Uuid::new_v4().to_string();
        let activated = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let path = self
            .config
            .private_directory
            .join(format!("session-{session_id}.sqlite3"));
        let master = credentials::create_fresh_master(OWNER)?;
        let store = credentials::encrypted_store(&path, &master, OWNER).await?;
        let phone_db = phone.clone();
        let epoch_db = epoch.clone();
        let activated_db = activated.clone();
        self.db(move|c|{let tx=c.transaction()?;
            tx.execute("INSERT INTO whatsapp(user_id,enabled,phone,status,activated_at) VALUES(?1,1,?2,'connecting',?3) ON CONFLICT(user_id) DO UPDATE SET enabled=1,phone=?2,status='connecting',activated_at=?3,qr=NULL,qr_expires_at=NULL,error=NULL,updated_at=now()",params![OWNER,phone_db,activated_db])?;
            tx.execute("INSERT INTO native_whatsapp_control(user_id,epoch,session_id,expected_phone,activated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(user_id) DO UPDATE SET epoch=?2,session_id=?3,expected_phone=?4,activated_at=?5",params![OWNER,epoch_db,session_id,phone_db,activated_db])?;
            tx.commit()?;Ok(())}).await?;
        self.start(store, path, epoch, phone, activation_second(&activated)?)
            .await
    }
    async fn reconnect(&mut self) -> Result<()> {
        if self.config.offline {
            return Ok(());
        }
        if self.session.is_some() {
            return Ok(());
        }
        let saved=self.db(|c|Ok(c.query_row("SELECT n.epoch,n.session_id,n.expected_phone,n.activated_at FROM native_whatsapp_control n JOIN whatsapp w ON w.user_id=n.user_id WHERE n.user_id=?1 AND w.enabled=1 AND n.session_id IS NOT NULL",[OWNER],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?)).await?;
        if let Some((epoch, session_id, phone, activated)) = saved {
            uuid::Uuid::parse_str(&session_id)
                .context("Invalid native WhatsApp session identity")?;
            let path = self
                .config
                .private_directory
                .join(format!("session-{session_id}.sqlite3"));
            ensure!(
                path.is_file(),
                "Native WhatsApp session is missing; disconnect and pair again"
            );
            let master = credentials::load_master(OWNER)?;
            let store = credentials::encrypted_store(&path, &master, OWNER).await?;
            self.start(store, path, epoch, phone, activation_second(&activated)?)
                .await?;
        }
        Ok(())
    }
    async fn start(
        &mut self,
        store: whatsapp_rust_sqlite_storage::SqliteStore,
        path: PathBuf,
        epoch: String,
        phone: String,
        activated_at: i64,
    ) -> Result<()> {
        let active = Arc::new(AtomicBool::new(true));
        let (notices, receiver) = mpsc::channel(64);
        let qr_tx = notices.clone();
        let qr_epoch = epoch.clone();
        let connected_tx = notices.clone();
        let connected_epoch = epoch.clone();
        let event_epoch = epoch.clone();
        let hook = IntakeHook {
            config: self.config.clone(),
            epoch: epoch.clone(),
            phone: phone.clone(),
            activated_at,
            active: active.clone(),
            changed: self.changed.clone(),
        };
        let bot = Bot::builder()
            .with_backend(store)
            .skip_history_sync()
            .with_event_delivery(EventDelivery::Ordered { capacity: 128 })
            .with_inbound_durability_hook(hook)
            .on_qr_code(move |code, expiry| {
                let sender = qr_tx.clone();
                let epoch = qr_epoch.clone();
                async move {
                    let _ = sender
                        .send(Notice::Qr {
                            epoch,
                            code,
                            expiry,
                        })
                        .await;
                }
            })
            .on_connected(move |client| {
                let sender = connected_tx.clone();
                let epoch = connected_epoch.clone();
                async move {
                    let _ = sender
                        .send(Notice::Connected {
                            epoch,
                            pn: client.pn().map(|jid| jid.to_string()),
                            lid: client.lid().map(|jid| jid.to_string()),
                        })
                        .await;
                }
            })
            .on_event(move |event, _client| {
                let sender = notices.clone();
                let epoch = event_epoch.clone();
                async move {
                    let notice = match &*event {
                        Event::Disconnected(_) => Some(Notice::Disconnected { epoch }),
                        Event::LoggedOut(_) => Some(Notice::LoggedOut { epoch }),
                        _ => None,
                    };
                    if let Some(notice) = notice {
                        let _ = sender.send(notice).await;
                    }
                }
            })
            .build()
            .await?;
        bot.client()
            .enable_auto_reconnect
            .store(true, Ordering::Release);
        self.session = Some(Session {
            epoch,
            path,
            phone: phone.clone(),
            activated_at,
            active,
            handle: bot.spawn(),
            notices: receiver,
        });
        self.view = WhatsAppView {
            state: ConnectionState::Connecting,
            phone: Some(phone),
            ..Default::default()
        };
        self.publish();
        Ok(())
    }
    async fn notice(&mut self, notice: Notice) -> Result<()> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        let notice_epoch = match &notice {
            Notice::Qr { epoch, .. }
            | Notice::Connected { epoch, .. }
            | Notice::Disconnected { epoch }
            | Notice::LoggedOut { epoch } => epoch,
        };
        if notice_epoch != &session.epoch {
            return Ok(());
        }
        let epoch = session.epoch.clone();
        let phone = session.phone.clone();
        let activated_at = session.activated_at;
        match notice {
            Notice::Qr { code, expiry, .. } => {
                self.view.state = ConnectionState::Pairing;
                self.view.qr = Some(code);
                self.view.qr_expires_at =
                    Some(chrono::Utc::now().timestamp() + expiry.as_secs() as i64);
                self.view.error = None;
            }
            Notice::Connected { pn, lid, .. } => {
                if let Err(error) = VerifiedOwner::verify(
                    &phone,
                    pn.as_deref().unwrap_or_default(),
                    lid.as_deref(),
                    activated_at,
                ) {
                    self.disconnect(true).await?;
                    bail!("{error}");
                }
                self.view.state = ConnectionState::Connected;
                self.view.qr = None;
                self.view.qr_expires_at = None;
                self.view.error = None;
            }
            Notice::Disconnected { .. } => {
                self.view.state = ConnectionState::Connecting;
                self.view.qr = None;
                self.view.qr_expires_at = None;
            }
            Notice::LoggedOut { .. } => {
                self.disconnect(true).await?;
                return Ok(());
            }
        }
        let state = self.view.state.sql().to_string();
        self.db(move|c|{c.execute("UPDATE whatsapp SET status=?3,qr=NULL,qr_expires_at=NULL,error=NULL,updated_at=now() WHERE user_id=?1 AND enabled=1 AND EXISTS(SELECT 1 FROM native_whatsapp_control n WHERE n.user_id=?1 AND n.epoch=?2)",params![OWNER,epoch,state])?;Ok(())}).await?;
        self.publish();
        if self.view.state == ConnectionState::Connected {
            self.send_receipts().await?;
        }
        Ok(())
    }
    async fn send_receipts(&mut self) -> Result<()> {
        if self.view.state != ConnectionState::Connected {
            return Ok(());
        }
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        let epoch = session.epoch.clone();
        let pending=self.db(move|c|Ok(c.prepare("SELECT r.message_key,r.ack_id,r.created_article FROM whatsapp_receipts r JOIN whatsapp w ON w.user_id=r.user_id JOIN native_whatsapp_control n ON n.user_id=w.user_id WHERE r.user_id=?1 AND w.enabled=1 AND n.epoch=?2 AND r.sent_at IS NULL AND julianday(r.created_at)>=julianday(n.activated_at) ORDER BY r.created_at,r.message_key LIMIT 10")?.query_map(params![OWNER,epoch],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)).await?;
        for (key, ack, created) in pending {
            ensure!(
                session.active.load(Ordering::Acquire),
                "WhatsApp session stopped"
            );
            let owner = format!("{}@s.whatsapp.net", session.phone).parse::<Jid>()?;
            let message = whatsapp_rust::waproto::whatsapp::Message::text(if created {
                "Saved to reading list"
            } else {
                "Already in reading list"
            });
            let options = SendOptions::default().with_message_id(ack.clone());
            tokio::time::timeout(
                Duration::from_secs(15),
                session
                    .handle
                    .client()
                    .send_message_with_options(owner, message, options),
            )
            .await
            .context("Reply timed out; it will retry")??;
            let epoch = session.epoch.clone();
            self.db(move|c|{c.execute("UPDATE whatsapp_receipts SET sent_at=now() WHERE user_id=?1 AND message_key=?2 AND sent_at IS NULL AND EXISTS(SELECT 1 FROM native_whatsapp_control n JOIN whatsapp w ON w.user_id=n.user_id WHERE n.user_id=?1 AND n.epoch=?3 AND w.enabled=1)",params![OWNER,key,epoch])?;Ok(())}).await?;
        }
        Ok(())
    }
    async fn disconnect(&mut self, erase: bool) -> Result<()> {
        if erase {
            self.db(|c|{let tx=c.transaction()?;tx.execute("UPDATE whatsapp SET enabled=0,status='disconnecting',qr=NULL,qr_expires_at=NULL,updated_at=now() WHERE user_id=?1",[OWNER])?;tx.execute("UPDATE native_whatsapp_control SET epoch=?2 WHERE user_id=?1",params![OWNER,uuid::Uuid::new_v4().to_string()])?;tx.commit()?;Ok(())}).await?;
        }
        let mut session_path = None;
        if let Some(session) = self.session.take() {
            session.active.store(false, Ordering::Release);
            session_path = Some(session.path);
            if erase {
                let _ =
                    tokio::time::timeout(Duration::from_secs(15), session.handle.client().logout())
                        .await;
            }
            session.handle.shutdown().await;
        }
        if erase {
            if let Some(path) = session_path {
                credentials::erase_session_files(&path)?;
            } else {
                let saved = self
                    .db(|c| {
                        Ok(c.query_row(
                            "SELECT session_id FROM native_whatsapp_control WHERE user_id=?1",
                            [OWNER],
                            |r| r.get::<_, Option<String>>(0),
                        )
                        .optional()?
                        .flatten())
                    })
                    .await?;
                if let Some(id) = saved {
                    uuid::Uuid::parse_str(&id)?;
                    credentials::erase_session_files(
                        &self
                            .config
                            .private_directory
                            .join(format!("session-{id}.sqlite3")),
                    )?;
                }
            }
            credentials::erase_master(OWNER)?;
            self.db(|c|{let tx=c.transaction()?;tx.execute("UPDATE native_whatsapp_control SET session_id=NULL,expected_phone=NULL WHERE user_id=?1",[OWNER])?;tx.execute("UPDATE whatsapp SET enabled=0,status='disconnected',phone=NULL,qr=NULL,qr_expires_at=NULL,error=NULL,updated_at=now() WHERE user_id=?1",[OWNER])?;tx.commit()?;Ok(())}).await?;
        }
        self.view = WhatsAppView::default();
        self.publish();
        Ok(())
    }
}
