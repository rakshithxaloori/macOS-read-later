//! One disk budget shared by every writer using the same native data directory.
//!
//! Reservations use locked files. A crashed process releases its lock, allowing
//! the next writer to reclaim its claim without relying on PID reuse or clocks.
use crate::domain::CopyStatus;
use anyhow::{Context, Result, ensure};
use clap::Parser;
use fs2::FileExt;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const LIMIT_BYTES: u64 = 100_000_000;
pub const CACHE_BYTES: u64 = 20_000_000;
pub const DEFAULT_BUNDLE_BYTES: u64 = 45_000_000;
pub const SAFETY_BYTES: u64 = 2_000_000;
const INDEX_LIMIT: u64 = 2_000_000;
const TTL_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedMedia {
    pub id: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedArticle {
    pub html: String,
    pub media: Vec<CachedMedia>,
    pub status: CopyStatus,
    pub error: Option<String>,
    pub word_count: u32,
    pub fetched_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    pub managed_bytes: u64,
    pub bundle_bytes: u64,
    pub reserved_bytes: u64,
    pub cache_bytes: u64,
    pub limit_bytes: u64,
}
impl Usage {
    pub fn total_bytes(&self) -> u64 {
        self.managed_bytes.saturating_add(self.bundle_bytes)
    }
    pub fn available_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(
            self.total_bytes()
                .saturating_add(self.reserved_bytes)
                .saturating_add(SAFETY_BYTES),
        )
    }
}
#[derive(Clone, Debug)]
pub struct Storage {
    root: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Claim {
    bytes: u64,
    kind: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    clock: i64,
    entries: BTreeMap<String, Entry>,
    generations: BTreeMap<String, u64>,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    bytes: u64,
    touched_at: i64,
    expires_at: i64,
}
#[derive(Serialize, Deserialize)]
struct DiskArticle {
    html: String,
    media: Vec<DiskMedia>,
    status: CopyStatus,
    error: Option<String>,
    word_count: u32,
    fetched_at: String,
}
#[derive(Serialize, Deserialize)]
struct DiskMedia {
    id: String,
    mime_type: String,
    path: String,
    sha256: String,
}

fn read_cached(directory: &Path) -> Result<CachedArticle> {
    ensure!(
        fs::symlink_metadata(directory)?.file_type().is_dir(),
        "Cached articles must be regular directories"
    );
    ensure!(
        measured_bytes(directory)? <= CACHE_BYTES,
        "Cached directory exceeds its bound"
    );
    let metadata = directory.join("article.json");
    ensure!(
        fs::symlink_metadata(&metadata)?.file_type().is_file(),
        "Invalid cached article metadata"
    );
    ensure!(
        fs::metadata(&metadata)?.len() <= CACHE_BYTES,
        "Cached metadata exceeds bound"
    );
    let disk: DiskArticle = serde_json::from_reader(File::open(metadata)?)?;
    let mut media = Vec::new();
    let mut read_bytes = disk.html.len() as u64;
    for value in disk.media {
        ensure!(
            value.path.bytes().all(|b| b.is_ascii_digit()) && !value.path.is_empty(),
            "Invalid cached media path"
        );
        let path = directory.join(value.path);
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file()
                && fs::metadata(&path)?.len() <= 5_242_880,
            "Invalid cached image"
        );
        read_bytes = read_bytes.saturating_add(fs::metadata(&path)?.len());
        ensure!(
            read_bytes <= CACHE_BYTES,
            "Cached article exceeds its memory bound"
        );
        let bytes = fs::read(path)?;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == value.sha256,
            "Corrupt cached image checksum"
        );
        media.push(CachedMedia {
            id: value.id,
            mime_type: value.mime_type,
            bytes,
        });
    }
    Ok(CachedArticle {
        html: disk.html,
        media,
        status: disk.status,
        error: disk.error,
        word_count: disk.word_count,
        fetched_at: disk.fetched_at,
    })
}

pub struct Reservation {
    storage: Storage,
    path: PathBuf,
    file: File,
    bytes: u64,
}
impl Reservation {
    /// Check growing work before publishing it. Claims deliberately overcount
    /// partially written work so concurrent writers can never spend its space.
    pub fn check(&self, actual_bytes: u64) -> Result<()> {
        ensure!(
            actual_bytes <= self.bytes,
            "Work exceeded its storage reservation"
        );
        let usage = self.storage.snapshot()?;
        ensure!(
            usage.total_bytes() < LIMIT_BYTES,
            "Read Later reached its 100 MB storage limit"
        );
        Ok(())
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn finish(self) -> Result<()> {
        self.check(self.bytes)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        // Removing a live, locked claim is safe under the global quota lock.
        if let Ok(_guard) = self.storage.lock() {
            let _ = fs::remove_file(&self.path);
        }
        let _ = FileExt::unlock(&self.file);
    }
}

fn regular_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.file_type().is_dir(),
            "Storage must use regular directories"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error.into()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn private_file(path: &Path) -> Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.file_type().is_file(),
            "Storage files must never be symlinks"
        );
    }
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?)
}
pub fn database_path(connection: &Connection) -> Result<Option<PathBuf>> {
    let path: String = connection.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |r| r.get(0),
    )?;
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}
pub fn measured_bytes(path: &Path) -> Result<u64> {
    match fs::symlink_metadata(path) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    }
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        ensure!(
            !entry.file_type().is_symlink(),
            "Managed storage contains a symlink"
        );
        if entry.file_type().is_file() {
            let metadata = entry.metadata()?;
            let occupied = metadata.len().max(metadata.blocks().saturating_mul(512));
            bytes = bytes
                .checked_add(occupied)
                .context("Storage byte count overflow")?;
        }
    }
    Ok(bytes)
}

/// Read-only accounting of actual allocated files. This command does not
/// initialize storage, evict cache entries, or reclaim stale reservations.
#[derive(Debug, Serialize)]
pub struct Audit {
    pub data_root: PathBuf,
    pub bundle: PathBuf,
    pub bundle_bytes: u64,
    pub managed_bytes: u64,
    pub total_bytes: u64,
    pub limit_bytes: u64,
    pub below_limit: bool,
    pub managed_breakdown: BTreeMap<String, u64>,
}

pub fn audit(root: &Path, bundle: &Path) -> Result<Audit> {
    ensure!(
        root.is_absolute() && bundle.is_absolute(),
        "Storage audit requires absolute data and bundle paths"
    );
    ensure!(
        fs::symlink_metadata(root)?.file_type().is_dir(),
        "Storage audit requires an existing regular data directory"
    );
    let root = root.canonicalize()?;
    let bundle_metadata = fs::symlink_metadata(bundle)?;
    ensure!(
        bundle_metadata.is_dir() || bundle_metadata.is_file(),
        "Storage audit requires an existing regular bundle"
    );
    let bundle = bundle.canonicalize()?;
    ensure!(
        !bundle.starts_with(&root) && !root.starts_with(&bundle),
        "App bundle and managed data must have separate accounting roots"
    );
    let mut managed_breakdown = BTreeMap::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        managed_breakdown.insert(
            entry.file_name().to_string_lossy().into_owned(),
            measured_bytes(&entry.path())?,
        );
    }
    let managed_bytes = managed_breakdown.values().try_fold(0u64, |sum, bytes| {
        sum.checked_add(*bytes)
            .context("Storage byte count overflow")
    })?;
    let bundle_bytes = measured_bytes(&bundle)?;
    let total_bytes = managed_bytes
        .checked_add(bundle_bytes)
        .context("Storage byte count overflow")?;
    Ok(Audit {
        data_root: root,
        bundle,
        bundle_bytes,
        managed_bytes,
        total_bytes,
        limit_bytes: LIMIT_BYTES,
        below_limit: total_bytes < LIMIT_BYTES,
        managed_breakdown,
    })
}

pub fn audit_cli(args: &[String]) -> Result<()> {
    #[derive(Parser)]
    struct Options {
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long)]
        bundle: PathBuf,
    }
    let options = Options::try_parse_from(
        std::iter::once("storage-audit".to_owned()).chain(args.iter().cloned()),
    )?;
    let report = audit(&options.data_root, &options.bundle)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(report.below_limit, "Installed app and data exceed 100 MB");
    Ok(())
}

fn bytes_in(path: &Path) -> Result<u64> {
    measured_bytes(path)
}
fn key(id: &str) -> String {
    format!("{:x}", Sha256::digest(id.as_bytes()))
}

impl Storage {
    pub fn new(root: impl AsRef<Path>, bundle_bytes: u64) -> Result<Self> {
        ensure!(
            bundle_bytes.saturating_add(SAFETY_BYTES) < LIMIT_BYTES,
            "App bundle leaves no room for its library"
        );
        let root = root.as_ref();
        ensure!(root.is_absolute(), "Managed storage path must be absolute");
        regular_directory(root)?;
        let value = Self {
            root: root.to_path_buf(),
        };
        regular_directory(&value.cache())?;
        regular_directory(&value.claims())?;
        let _guard = value.lock()?;
        if !value.root.join("storage-policy.json").exists() {
            value.write_json(&value.root.join("storage-policy.json"), &bundle_bytes)?;
        }
        value.recover_locked()?;
        Ok(value)
    }
    pub fn for_database(connection: &Connection) -> Result<Self> {
        let path =
            database_path(connection)?.context("In-memory databases have no disk storage owner")?;
        Self::new(
            path.parent().context("Database parent missing")?,
            DEFAULT_BUNDLE_BYTES,
        )
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn cache(&self) -> PathBuf {
        self.root.join("article-cache")
    }
    fn claims(&self) -> PathBuf {
        self.root.join("storage-reservations")
    }
    fn index_path(&self) -> PathBuf {
        self.root.join("article-cache-index.json")
    }
    fn lock(&self) -> Result<File> {
        let file = private_file(&self.root.join("storage.lock"))?;
        file.lock_exclusive()?;
        Ok(file)
    }
    fn write_json(&self, path: &Path, value: &impl Serialize) -> Result<()> {
        let temp = path.with_extension("pending");
        let mut file = private_file(&temp)?;
        file.set_len(0)?;
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(temp, path)?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    fn read_index(&self) -> Result<Index> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(Index::default());
        }
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file(),
            "Cache index must be a regular file"
        );
        ensure!(
            fs::metadata(&path)?.len() <= INDEX_LIMIT,
            "Cache index exceeds its bound"
        );
        let mut value: Index = serde_json::from_reader(File::open(path)?)?;
        for entry in value.entries.values() {
            ensure!(
                entry.path.len() == 64 && entry.path.bytes().all(|v| v.is_ascii_hexdigit()),
                "Invalid cache path"
            );
        }
        value.clock = value.clock.max(
            value
                .entries
                .values()
                .map(|e| e.touched_at)
                .max()
                .unwrap_or(0),
        );
        Ok(value)
    }
    fn save_index(&self, index: &Index) -> Result<()> {
        ensure!(
            serde_json::to_vec(index)?.len() as u64 <= INDEX_LIMIT,
            "Cache index exceeds its bound"
        );
        self.write_json(&self.index_path(), index)
    }
    fn claimed_locked(&self) -> Result<u64> {
        let mut total = 0u64;
        for entry in fs::read_dir(self.claims())? {
            let path = entry?.path();
            let file = private_file(&path)?;
            match file.try_lock_exclusive() {
                Ok(()) => {
                    fs::remove_file(path)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    ensure!(
                        file.metadata()?.len() <= 4096,
                        "Invalid storage reservation"
                    );
                    let claim: Claim = serde_json::from_reader(file)?;
                    total = total
                        .checked_add(claim.bytes)
                        .context("Reservation overflow")?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(total)
    }
    fn snapshot_locked(&self) -> Result<Usage> {
        let reserved_bytes = self.claimed_locked()?;
        let bundle_bytes =
            serde_json::from_reader(File::open(self.root.join("storage-policy.json"))?)?;
        Ok(Usage {
            managed_bytes: bytes_in(&self.root)?,
            bundle_bytes,
            reserved_bytes,
            cache_bytes: bytes_in(&self.cache())?,
            limit_bytes: LIMIT_BYTES,
        })
    }
    pub fn snapshot(&self) -> Result<Usage> {
        let _guard = self.lock()?;
        self.snapshot_locked()
    }
    /// Use the measured installed bundle size, never a debug or development build.
    pub fn set_bundle_bytes(&self, bytes: u64) -> Result<()> {
        ensure!(
            bytes.saturating_add(SAFETY_BYTES) < LIMIT_BYTES,
            "App bundle leaves no library budget"
        );
        let _guard = self.lock()?;
        let old = self.snapshot_locked()?;
        let mut index = self.read_index()?;
        self.evict_locked(&mut index, bytes.saturating_sub(old.bundle_bytes))?;
        let current = self.snapshot_locked()?;
        ensure!(
            current
                .managed_bytes
                .saturating_add(bytes)
                .saturating_add(current.reserved_bytes)
                .saturating_add(SAFETY_BYTES)
                < LIMIT_BYTES,
            "Installed bundle and durable library exceed the storage budget"
        );
        self.write_json(&self.root.join("storage-policy.json"), &bytes)
    }
    fn remove_entry(&self, index: &mut Index, id: &str) -> Result<()> {
        if let Some(entry) = index.entries.remove(id) {
            let path = self.cache().join(entry.path);
            if path.exists() {
                fs::remove_dir_all(path)?;
            }
        }
        Ok(())
    }
    fn evict_locked(&self, index: &mut Index, needed: u64) -> Result<()> {
        let mut order: Vec<_> = index
            .entries
            .iter()
            .map(|(id, e)| (e.touched_at, id.clone()))
            .collect();
        order.sort();
        for (_, id) in order {
            if self.snapshot_locked()?.available_bytes() >= needed {
                break;
            }
            self.remove_entry(index, &id)?;
        }
        self.save_index(index)?;
        Ok(())
    }
    fn recover_locked(&self) -> Result<()> {
        self.claimed_locked()?;
        let mut index = self.read_index()?;
        let now = chrono::Utc::now().timestamp();
        let expired: Vec<_> = index
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at <= now || !self.cache().join(&e.path).is_dir())
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.remove_entry(&mut index, &id)?;
        }
        // Atomic cache directories are named by digest; incomplete writes are
        // unindexed and can always be removed after taking the quota lock.
        for entry in fs::read_dir(self.cache())? {
            let path = entry?.path();
            if !index
                .entries
                .values()
                .any(|e| self.cache().join(&e.path) == path)
            {
                ensure!(
                    fs::symlink_metadata(&path)?.file_type().is_dir(),
                    "Invalid orphan cache entry"
                );
                fs::remove_dir_all(path)?;
            }
        }
        self.save_index(&index)?;
        Ok(())
    }
    pub fn require(&self, bytes: u64) -> Result<Reservation> {
        self.reserve("work", bytes)
    }
    pub fn reserve(&self, kind: &str, bytes: u64) -> Result<Reservation> {
        ensure!(
            bytes < LIMIT_BYTES,
            "Requested work cannot fit below 100 MB"
        );
        let _guard = self.lock()?;
        let mut index = self.read_index()?;
        self.evict_locked(&mut index, bytes)?;
        ensure!(
            self.snapshot_locked()?.available_bytes() >= bytes,
            "Storage budget exhausted. Export or remove a finalized PDF before adding more data"
        );
        let path = self.claims().join(uuid::Uuid::new_v4().to_string());
        let mut file = private_file(&path)?;
        file.lock_exclusive()?;
        serde_json::to_writer(
            &mut file,
            &Claim {
                bytes,
                kind: kind.chars().take(80).collect(),
            },
        )?;
        file.sync_all()?;
        Ok(Reservation {
            storage: self.clone(),
            path,
            file,
            bytes,
        })
    }
    pub fn cache_generation(&self, article_id: &str) -> Result<u64> {
        let _guard = self.lock()?;
        Ok(*self.read_index()?.generations.get(article_id).unwrap_or(&0))
    }
    /// Reclaim disposable payloads before reserving complete magazine work.
    pub fn clear_cache(&self) -> Result<()> {
        let _guard = self.lock()?;
        let mut index = self.read_index()?;
        let ids: Vec<_> = index.entries.keys().cloned().collect();
        for id in ids {
            self.remove_entry(&mut index, &id)?;
        }
        self.save_index(&index)
    }
    pub fn invalidate(&self, article_id: &str) -> Result<u64> {
        let _guard = self.lock()?;
        let mut index = self.read_index()?;
        self.remove_entry(&mut index, article_id)?;
        let next = index
            .generations
            .get(article_id)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .context("Cache generation overflow")?;
        index.generations.insert(article_id.to_owned(), next);
        self.save_index(&index)?;
        Ok(next)
    }
    pub fn cache_get(&self, article_id: &str) -> Result<Option<CachedArticle>> {
        let _guard = self.lock()?;
        let mut index = self.read_index()?;
        index.clock = index
            .clock
            .checked_add(1)
            .context("Cache access clock overflow")?;
        let Some(entry) = index.entries.get_mut(article_id) else {
            return Ok(None);
        };
        let now = chrono::Utc::now().timestamp();
        if entry.expires_at <= now || !self.cache().join(&entry.path).is_dir() {
            self.remove_entry(&mut index, article_id)?;
            self.save_index(&index)?;
            return Ok(None);
        }
        ensure!(entry.bytes <= CACHE_BYTES, "Cache entry exceeds bound");
        let directory = self.cache().join(&entry.path);
        let copy = match read_cached(&directory) {
            Ok(copy) => copy,
            Err(_) => {
                // Cached copies are disposable. A damaged image or partial
                // directory is a miss, preserving article quality and state.
                self.remove_entry(&mut index, article_id)?;
                self.save_index(&index)?;
                return Ok(None);
            }
        };
        entry.touched_at = index.clock;
        self.save_index(&index)?;
        Ok(Some(copy))
    }
    pub fn cache_put(
        &self,
        article_id: &str,
        epoch: u64,
        archived: bool,
        article: CachedArticle,
    ) -> Result<bool> {
        if archived {
            return Ok(false);
        }
        let _guard = self.lock()?;
        let mut index = self.read_index()?;
        if index.generations.get(article_id).copied().unwrap_or(0) != epoch {
            return Ok(false);
        }
        let mut disk = DiskArticle {
            html: article.html,
            media: Vec::new(),
            status: article.status,
            error: article.error,
            word_count: article.word_count,
            fetched_at: article.fetched_at,
        };
        let mut bytes = 0u64;
        for (number, media) in article.media.iter().enumerate() {
            ensure!(media.bytes.len() <= 5_242_880, "Cached image exceeds 5 MB");
            bytes = bytes
                .checked_add(media.bytes.len() as u64)
                .context("Cache bytes overflow")?;
            disk.media.push(DiskMedia {
                id: media.id.clone(),
                mime_type: media.mime_type.clone(),
                path: number.to_string(),
                sha256: format!("{:x}", Sha256::digest(&media.bytes)),
            });
        }
        let metadata = serde_json::to_vec(&disk)?;
        bytes = bytes
            .checked_add(metadata.len() as u64)
            .context("Cache bytes overflow")?;
        let occupied_bound = bytes.saturating_add(
            (article.media.len() as u64)
                .saturating_add(1)
                .saturating_mul(4096),
        );
        if occupied_bound > CACHE_BYTES {
            return Ok(false);
        }
        self.remove_entry(&mut index, article_id)?;
        let mut order: Vec<_> = index
            .entries
            .iter()
            .map(|(id, e)| (e.touched_at, id.clone()))
            .collect();
        order.sort();
        for (_, id) in order {
            if bytes_in(&self.cache())?.saturating_add(occupied_bound) <= CACHE_BYTES {
                break;
            }
            self.remove_entry(&mut index, &id)?;
        }
        self.evict_locked(&mut index, occupied_bound.saturating_add(INDEX_LIMIT))?;
        if self.snapshot_locked()?.available_bytes() < occupied_bound.saturating_add(INDEX_LIMIT) {
            return Ok(false);
        }
        let name = key(article_id);
        let directory = self.cache().join(&name);
        if directory.exists() {
            ensure!(
                fs::symlink_metadata(&directory)?.file_type().is_dir(),
                "Invalid interrupted cache directory"
            );
            fs::remove_dir_all(&directory)?;
        }
        regular_directory(&directory)?;
        // The lock prevents readers seeing partially written directories; an
        // interrupted directory remains unindexed and is reclaimed on restart.
        for (number, media) in article.media.iter().enumerate() {
            let mut file = private_file(&directory.join(number.to_string()))?;
            file.set_len(0)?;
            file.write_all(&media.bytes)?;
            file.sync_all()?;
        }
        let mut file = private_file(&directory.join("article.json"))?;
        file.set_len(0)?;
        file.write_all(&metadata)?;
        file.sync_all()?;
        let now = chrono::Utc::now().timestamp();
        index.clock = index
            .clock
            .checked_add(1)
            .context("Cache access clock overflow")?;
        index.entries.insert(
            article_id.to_owned(),
            Entry {
                path: name,
                bytes,
                touched_at: index.clock,
                expires_at: now + TTL_SECONDS,
            },
        );
        self.save_index(&index)?;
        ensure!(
            self.snapshot_locked()?.total_bytes() < LIMIT_BYTES,
            "Cache publication exceeded storage budget"
        );
        Ok(true)
    }
}
