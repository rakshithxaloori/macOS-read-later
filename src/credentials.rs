//! Native-only WhatsApp credentials: a fresh Keychain master and owner-derived SQLCipher key.
//! The legacy Baileys encryption key is never read.
use anyhow::{Context, Result, ensure};
use std::{path::Path, time::Duration};
use whatsapp_rust_sqlite_storage::{SqliteStore, SqliteStoreConfig, Synchronous};
use zeroize::Zeroizing;

const KEYCHAIN_SERVICE: &str = "org.reading-list.native.whatsapp.v1";

pub fn derive_owner_key(master: &[u8; 32], owner: &str) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0; 32]);
    hkdf::Hkdf::<sha2::Sha256>::new(Some(KEYCHAIN_SERVICE.as_bytes()), master)
        .expand(owner.as_bytes(), key.as_mut())
        .expect("fixed 32-byte HKDF output");
    key
}

#[cfg(target_os = "macos")]
pub fn load_master(owner: &str) -> Result<Zeroizing<[u8; 32]>> {
    let value = Zeroizing::new(
        security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, owner)
            .context("Native WhatsApp key is unavailable in macOS Keychain")?,
    );
    ensure!(value.len() == 32, "Invalid native WhatsApp encryption key");
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&value);
    Ok(key)
}

#[cfg(target_os = "macos")]
pub fn create_fresh_master(owner: &str) -> Result<Zeroizing<[u8; 32]>> {
    let mut master = Zeroizing::new([0; 32]);
    security_framework::random::SecRandom::default().copy_bytes(master.as_mut())?;
    security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, owner, master.as_ref())
        .context("Cannot store the native WhatsApp key in macOS Keychain")?;
    Ok(master)
}

#[cfg(target_os = "macos")]
pub fn erase_master(owner: &str) -> Result<()> {
    match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, owner) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()), // errSecItemNotFound
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn load_master(_owner: &str) -> Result<Zeroizing<[u8; 32]>> {
    anyhow::bail!("Native credential storage requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn create_fresh_master(_owner: &str) -> Result<Zeroizing<[u8; 32]>> {
    anyhow::bail!("Native credential storage requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn erase_master(_owner: &str) -> Result<()> {
    anyhow::bail!("Native credential storage requires macOS")
}

/// Protect private storage before creating a session. Refuse symlink substitution.
pub fn prepare_private_directory(directory: &Path) -> Result<()> {
    if directory.exists() {
        let metadata = std::fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Invalid private credential directory"
        );
    } else {
        std::fs::create_dir_all(directory)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub async fn encrypted_store(path: &Path, master: &[u8; 32], owner: &str) -> Result<SqliteStore> {
    use diesel::prelude::*;
    #[derive(QueryableByName)]
    struct CipherVersion {
        #[diesel(sql_type = diesel::sql_types::Text)]
        cipher_version: String,
    }
    let directory = path.parent().context("Invalid session path")?;
    let budget_guard = path
        .ancestors()
        .find(|p| p.join("storage-policy.json").is_file())
        .map(|root| {
            crate::storage::Storage::new(root, crate::storage::DEFAULT_BUNDLE_BYTES)
                .and_then(|s| s.reserve("native credentials", 12_000_000))
        })
        .transpose()?
        .map(std::sync::Arc::new);
    prepare_private_directory(directory)?;
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Invalid native session file"
        );
    } else {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)?.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let key = derive_owner_key(master, owner);
    let config = SqliteStoreConfig {
        busy_timeout: Duration::from_secs(10),
        synchronous: Synchronous::Full,
        ..Default::default()
    }
    .with_connection_init(move |connection| {
        if let Some(guard) = &budget_guard {
            guard.check(0)?;
        }
        diesel::sql_query(format!("PRAGMA key = \"x'{}'\"", hex::encode(key.as_ref())))
            .execute(connection)?;
        let version =
            diesel::sql_query("PRAGMA cipher_version").get_result::<CipherVersion>(connection)?;
        if version.cipher_version.is_empty() {
            return Err("SQLCipher is unavailable".into());
        }
        diesel::sql_query("SELECT count(*) FROM sqlite_master").execute(connection)?;
        if budget_guard.is_some() {
            for pragma in [
                "PRAGMA max_page_count=1024",
                "PRAGMA wal_autocheckpoint=64",
                "PRAGMA journal_size_limit=1048576",
            ] {
                diesel::sql_query(pragma).execute(connection)?;
            }
        }
        Ok(())
    });
    Ok(SqliteStore::with_config(
        path.to_str().context("Invalid native session path")?,
        config,
    )
    .await?)
}

pub fn erase_session_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_os_string();
        file.push(suffix);
        match std::fs::remove_file(Path::new(&file)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
