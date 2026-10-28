//! SQLite functions matching Reading List Web's retained schema.
use chrono::{DateTime, SecondsFormat, Utc};
use md5::{Digest, Md5};
use rusqlite::{Connection, OpenFlags, functions::FunctionFlags};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::OnceLock, time::Duration};

/// The single local owner row required by the imported schema.
pub const OWNER: &str = "00000000-0000-4000-8000-000000000001";

pub fn register(connection: &Connection) -> rusqlite::Result<()> {
    let deterministic = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    connection.create_scalar_function("now", 0, FunctionFlags::SQLITE_UTF8, |_| {
        Ok(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
    })?;
    connection.create_scalar_function("md5", 1, deterministic, |ctx| {
        Ok(format!(
            "{:x}",
            Md5::digest(ctx.get::<String>(0)?.as_bytes())
        ))
    })?;
    connection.create_scalar_function("strip_html", 1, deterministic, |ctx| {
        let value = ctx.get::<String>(0)?;
        // Same literal <...> replacement as the web schema, including multiline tags.
        static TAGS: OnceLock<regex::Regex> = OnceLock::new();
        Ok(TAGS
            .get_or_init(|| regex::Regex::new("<[^>]*>").expect("constant pattern"))
            .replace_all(&value, " ")
            .into_owned())
    })?;
    connection.create_scalar_function("regexp", 2, deterministic, |ctx| {
        let pattern = ctx.get::<String>(0)?;
        let value = ctx.get::<String>(1)?;
        let regex = regex::Regex::new(&pattern)
            .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
        Ok(i32::from(regex.is_match(&value)))
    })?;
    connection.create_scalar_function("publication_hostname", 1, deterministic, |ctx| {
        let host = url::Url::parse(&ctx.get::<String>(0)?)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        Ok(host.strip_prefix("www.").unwrap_or(&host).to_owned())
    })?;
    connection.create_scalar_function("date_add", -1, FunctionFlags::SQLITE_UTF8, |ctx| {
        let time = ctx.get::<String>(0)?;
        let amount = ctx.get::<f64>(1)?;
        let unit = if ctx.len() >= 3 {
            ctx.get::<String>(2)?
        } else {
            "seconds".into()
        };
        let factor = match unit.as_str() {
            "second" | "seconds" => 1000.,
            "minute" | "minutes" => 60_000.,
            "hour" | "hours" => 3_600_000.,
            "day" | "days" => 86_400_000.,
            _ => {
                return Err(rusqlite::Error::UserFunctionError(
                    "Unsupported SQLite date interval".into(),
                ));
            }
        };
        let value = DateTime::parse_from_rfc3339(&time)
            .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
        let delta = (amount * factor) as i64;
        let value = value
            .checked_add_signed(chrono::Duration::milliseconds(delta))
            .ok_or_else(|| rusqlite::Error::UserFunctionError("SQLite date overflow".into()))?;
        Ok(value
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true))
    })?;
    Ok(())
}

pub fn open_read_only(path: &Path) -> anyhow::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    register(&connection)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

pub fn open(path: &Path) -> anyhow::Result<Connection> {
    anyhow::ensure!(path.is_absolute(), "Native database path must be absolute");
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Native database requires a parent directory"))?;
    if parent.exists() {
        let metadata = fs::symlink_metadata(parent)?;
        anyhow::ensure!(
            metadata.file_type().is_dir() && metadata.permissions().mode() & 0o077 == 0,
            "Native database parent must be a private regular directory"
        );
    } else {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_file(),
            "Native database must be a regular file, never a symlink"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let connection = Connection::open(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    register(&connection)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA recursive_triggers=OFF;")?;
    if is_lean(&connection)? {
        configure_lean(&connection)?;
    }
    Ok(connection)
}

/// Initialize the baseline atomically and version native migrations separately.
/// Imported web migrations, library rows and finalized triggers stay intact.
pub fn initialize(connection: &Connection) -> anyhow::Result<()> {
    let transaction =
        rusqlite::Transaction::new_unchecked(connection, rusqlite::TransactionBehavior::Immediate)?;
    let exists: i64 = transaction.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='articles'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        transaction.execute_batch(include_str!("../schema/web-001.sql"))?;
        transaction.execute(
            "INSERT INTO users(id,email) VALUES(?1,'local@reading-list.invalid')",
            [OWNER],
        )?;
    }
    transaction.execute_batch("CREATE TABLE IF NOT EXISTS native_schema_migrations(version INTEGER PRIMARY KEY CHECK(version>0), applied_at TEXT NOT NULL DEFAULT(now())); INSERT OR IGNORE INTO native_schema_migrations(version) VALUES(1);")?;
    transaction.commit()?;
    if is_lean(connection)? {
        configure_lean(connection)?;
    }
    Ok(())
}

pub fn is_lean(connection: &Connection) -> anyhow::Result<bool> {
    let exists: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='native_storage_policy'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(false);
    }
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_storage_policy WHERE id=1 AND version=2)",
        [],
        |r| r.get(0),
    )?)
}

/// New stores opt in directly. A populated full-copy store is refused rather
/// than silently deleting its retained payloads.
pub fn initialize_lean(connection: &Connection) -> anyhow::Result<()> {
    let tables: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    if tables == 0 {
        connection.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;")?;
    }
    initialize(connection)?;
    if !is_lean(connection)? {
        let payloads: i64 = connection.query_row("SELECT (SELECT count(*) FROM articles WHERE html<>'') + (SELECT count(*) FROM article_media) + (SELECT count(*) FROM feed_captures WHERE processed_at IS NOT NULL) + (SELECT count(*) FROM magazine_articles WHERE html<>'') + (SELECT count(*) FROM magazine_artwork) + (SELECT count(*) FROM magazine_covers) + (SELECT count(*) FROM magazine_cover_requests WHERE original_front IS NOT NULL)", [], |r| r.get(0))?;
        anyhow::ensure!(
            payloads == 0,
            "Use verified compact conversion for an existing full-copy library"
        );
    }
    connection.execute_batch(include_str!("../schema/native-lean-002.sql"))?;
    let vacuum: i64 = connection.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
    if vacuum != 2 {
        connection.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")?;
    }
    configure_lean(connection)?;
    Ok(())
}

/// A bounded database and short WAL complement the quota owner's reservations.
/// Journal headroom is included in the global 100 MB ceiling.
pub fn configure_lean(connection: &Connection) -> anyhow::Result<()> {
    let page_size =
        u64::try_from(connection.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0))?)?;
    anyhow::ensure!(page_size > 0, "SQLite returned an invalid page size");
    let max_pages = 26_500_000 / page_size;
    connection.execute_batch(&format!("PRAGMA temp_store=MEMORY; PRAGMA wal_autocheckpoint=16; PRAGMA journal_size_limit=0; PRAGMA max_page_count={max_pages};"))?;
    Ok(())
}

/// Reclaim deleted temporary payloads without waiting for a reader's snapshot.
/// A busy reader defers reclamation to the next maintenance pass.
pub fn reclaim(connection: &Connection) -> anyhow::Result<()> {
    if !is_lean(connection)? || !connection.is_autocommit() {
        return Ok(());
    }
    let timeout =
        u64::try_from(connection.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))?)?;
    connection.busy_timeout(Duration::ZERO)?;
    let result = (|| -> anyhow::Result<()> {
        let busy: i64 =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        if busy != 0 {
            return Ok(());
        }
        connection.execute_batch("PRAGMA incremental_vacuum(512);")?;
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            r.get::<_, i64>(0)
        })?;
        Ok(())
    })();
    connection.busy_timeout(Duration::from_millis(timeout))?;
    match result {
        Err(error) if error.downcast_ref::<rusqlite::Error>().is_some_and(|e| matches!(e,rusqlite::Error::SqliteFailure(code,_) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))) => Ok(()),
        result => result,
    }
}
