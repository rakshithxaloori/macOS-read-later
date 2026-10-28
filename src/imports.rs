use crate::{domain::*, extraction, library, network::normalize_url};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::{Cursor, Read},
    path::Path,
};

const MAX_UPLOAD: usize = 20 * 1024 * 1024;
const MAX_ROWS: usize = 5000;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    Article(ArticleItem),
    Feed(FeedItem),
}
impl Item {
    fn identity(&self) -> (&str, &str) {
        match self {
            Self::Article(a) => ("article", &a.url),
            Self::Feed(f) => ("feed", &f.url),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleItem {
    pub key: String,
    pub url: String,
    pub title: String,
    pub author: Option<String>,
    pub site: String,
    pub html: String,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub word_count: u32,
    pub status: CopyStatus,
    pub saved_at: Option<String>,
    pub published_at: Option<String>,
    pub read: bool,
    pub archived: bool,
    pub read_at: Option<String>,
    pub archived_at: Option<String>,
    pub metadata: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeedItem {
    pub key: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub metadata: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Problem {
    pub row: usize,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub format: String,
    pub items: Vec<Item>,
    pub problems: Vec<Problem>,
    pub warnings: Vec<String>,
    pub duplicate_rows: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Progress {
    pub id: String,
    pub filename: String,
    pub format: String,
    pub state: String,
    pub total: u32,
    pub invalid: u32,
    pub pending: u32,
    pub added: u32,
    pub duplicates: u32,
    pub failed: u32,
    pub error: Option<String>,
    pub warnings: Value,
    pub validation_errors: Value,
    pub row_errors: Vec<Problem>,
    pub preview_rows: Vec<Item>,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn norm(s: &str) -> String {
    s.to_ascii_lowercase().replace([' ', '_', '-'], "")
}
fn string(v: Option<&Value>) -> String {
    v.map(|v| {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    })
    .unwrap_or_default()
}
fn bool_value(v: Option<&Value>, fallback: bool) -> Result<bool> {
    match v {
        None | Some(Value::Null) => Ok(fallback),
        Some(Value::Bool(b)) => Ok(*b),
        Some(v) => match string(Some(v)).to_ascii_lowercase().as_str() {
            "" => Ok(fallback),
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            _ => bail!("Invalid reading-state flag"),
        },
    }
}
fn date(v: Option<&Value>) -> Result<Option<String>> {
    let s = string(v);
    if s.is_empty() || s == "null" {
        return Ok(None);
    }
    let d = if s.len() == 10 && s.bytes().all(|x| x.is_ascii_digit()) {
        chrono::DateTime::from_timestamp(s.parse()?, 0).context("Invalid timestamp")?
    } else {
        chrono::DateTime::parse_from_rfc3339(&s)
            .or_else(|_| chrono::DateTime::parse_from_rfc2822(&s))?
            .with_timezone(&chrono::Utc)
    };
    Ok(Some(d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
}
fn article(record: Value, format: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<Item> {
    let obj = record.as_object().context("Invalid article record")?;
    let fields: BTreeMap<String, &Value> = obj.iter().map(|(k, v)| (norm(k), v)).collect();
    let get = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            fields
                .get(*k)
                .copied()
                .filter(|v| !v.is_null() && !string(Some(v)).is_empty())
        })
    };
    let url = normalize_url(&string(get(&["url", "link", "originalurl", "sourceurl"])))?;
    let path = string(get(&["fileid", "htmlpath"]));
    let raw = if path.is_empty() {
        string(get(&["html", "contenthtml"]))
    } else {
        files
            .get(&path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    };
    let html = extraction::clean_html(&raw, url.as_str(), false);
    let html = regex::Regex::new("(?is)<img\\b[^>]*>")?
        .replace_all(&html, "")
        .into_owned();
    ensure!(html.len() <= 2 * 1024 * 1024, "Article body exceeds 2 MB");
    let known = string(get(&["status", "contentstatus"]));
    let status = match known.as_str() {
        "saved" | "complete" => CopyStatus::Saved,
        "partial" => CopyStatus::Partial,
        "failed" => CopyStatus::Failed,
        _ if html.is_empty() => CopyStatus::Pending,
        _ => CopyStatus::Partial,
    };
    let read_at = date(get(&["readat"]))?;
    let archived_at = date(get(&["archivedat"]))?;
    let archived = bool_value(
        get(&["archived"]),
        archived_at.is_some()
            || (format == "matter"
                && get(&["inqueue"]).is_some()
                && !bool_value(get(&["inqueue"]), true)?),
    )?;
    let metadata: serde_json::Map<String, Value> = obj
        .iter()
        .filter(|(k, _)| !matches!(norm(k).as_str(), "html" | "contenthtml"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    ensure!(
        serde_json::to_vec(&metadata)?.len() <= 65536,
        "Metadata exceeds 64 KB"
    );
    let key = string(get(&["key", "id", "fileid"]));
    let key = if key.is_empty() {
        hash(url.as_str().as_bytes())
    } else {
        key
    };
    ensure!(key.len() <= 500, "Source identity too long");
    let title = string(get(&["title"]));
    let title = if title.is_empty() {
        url.as_str().into()
    } else {
        title
    };
    let site = string(get(&["site", "publisher", "sitename"]));
    let site = if site.is_empty() {
        url.host_str().unwrap_or_default().into()
    } else {
        site
    };
    let author = string(get(&["author"]));
    ensure!(
        title.len() <= 1000 && site.len() <= 1000 && author.len() <= 1000,
        "Metadata field exceeds 1,000 characters"
    );
    Ok(Item::Article(ArticleItem {
        key,
        url: url.to_string(),
        title,
        author: (!author.is_empty()).then_some(author),
        site,
        excerpt: if html.is_empty() {
            string(get(&["excerpt", "preview"]))
                .chars()
                .take(260)
                .collect()
        } else {
            extraction::text(&html).chars().take(260).collect()
        },
        word_count: if html.is_empty() {
            string(get(&["wordcount"])).parse().unwrap_or(0)
        } else {
            extraction::word_count(&html)
        },
        html,
        status,
        saved_at: date(get(&["savedat", "createdat", "timeadded", "adddate"]))?,
        published_at: date(get(&["publishedat", "publisheddate"]))?,
        read: bool_value(get(&["read", "isread"]), read_at.is_some())?,
        archived,
        read_at,
        archived_at,
        metadata: Value::Object(metadata),
    }))
}
pub fn preview(bytes: &[u8], filename: &str) -> Result<Preview> {
    ensure!(bytes.len() <= MAX_UPLOAD, "Upload exceeds 20 MB");
    let mut files = BTreeMap::new();
    let mut text = String::from_utf8_lossy(bytes)
        .trim_start_matches('\u{feff}')
        .to_owned();
    let mut format = "csv".to_owned();
    let mut warnings = vec![];
    if bytes.starts_with(b"PK\x03\x04") || filename.to_ascii_lowercase().ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        ensure!(archive.len() <= 10000, "Too many ZIP entries");
        let mut expanded = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_owned();
            ensure!(
                !name.contains(['\\', '\0'])
                    && !name.starts_with('/')
                    && !name.split('/').any(|p| p == ".." || p == ".")
                    && !files.contains_key(&name),
                "Unsafe or duplicate ZIP name"
            );
            ensure!(entry.size() <= 8 * 1024 * 1024, "ZIP member exceeds 8 MB");
            expanded += entry.size();
            ensure!(expanded <= 64 * 1024 * 1024, "ZIP expansion exceeds 64 MB");
            ensure!(
                matches!(
                    entry.compression(),
                    zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
                ),
                "Unsupported ZIP compression"
            );
            if entry.is_dir() {
                continue;
            }
            let mut body = Vec::new();
            entry
                .by_ref()
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut body)?;
            ensure!(
                body.len() as u64 == entry.size(),
                "Invalid ZIP member length"
            );
            files.insert(name, body);
        }
        if let Some(json) = files.get("reading-list.json") {
            text = String::from_utf8(json.clone())?;
            format = "reading-list".into();
        } else if let Some(history) = files
            .keys()
            .find(|p| p.ends_with("_matter_history.csv"))
            .cloned()
        {
            let prefix = history.trim_end_matches("_matter_history.csv").to_owned();
            files = files
                .into_iter()
                .map(|(k, v)| (k.strip_prefix(&prefix).unwrap_or(&k).to_owned(), v))
                .collect();
            text = String::from_utf8(
                files
                    .get("_matter_history.csv")
                    .context("Missing Matter history")?
                    .clone(),
            )?;
            format = "matter".into();
            warnings.push(
                "Matter bodies remain partial until verified against a publisher capture.".into(),
            );
        } else {
            bail!("ZIP must contain reading-list.json or _matter_history.csv");
        }
    }
    let mut items = vec![];
    let mut problems = vec![];
    let mut accept = |row: usize, result: Result<Item>| match result {
        Ok(item) => items.push(item),
        Err(e) => problems.push(Problem {
            row,
            message: e.to_string(),
        }),
    };
    if text.trim_start().starts_with(['{', '[']) {
        let manifest: Value = serde_json::from_str(&text)?;
        ensure!(
            manifest["format"] == "reading-list-v1",
            "Unsupported portable JSON format"
        );
        format = "reading-list".into();
        let articles = manifest["articles"]
            .as_array()
            .context("Missing articles array")?;
        let feeds = manifest["feeds"].as_array().cloned().unwrap_or_default();
        ensure!(
            articles.len() + feeds.len() <= MAX_ROWS,
            "Import exceeds 5,000 records"
        );
        for (i, row) in articles.iter().enumerate() {
            accept(i + 1, article(row.clone(), &format, &files));
        }
        for (i, row) in feeds.iter().enumerate() {
            let result = (|| {
                let url = normalize_url(&string(row.get("url")))?.to_string();
                Ok(Item::Feed(FeedItem {
                    key: {
                        let key = string(row.get("key"));
                        ensure!(key.len() <= 500, "Feed identity exceeds 500 bytes");
                        if key.trim().is_empty() {
                            url.clone()
                        } else {
                            key
                        }
                    },
                    url,
                    title: string(row.get("title")),
                    active: bool_value(row.get("active"), true)?,
                    metadata: row.clone(),
                }))
            })();
            accept(articles.len() + i + 1, result);
        }
        warnings.push("Portable imports restore articles and feeds. Exported magazine files and cached images remain in the archive.".into());
    } else if text.to_ascii_lowercase().contains("<opml") {
        ensure!(
            !regex::Regex::new("(?i)<!DOCTYPE|<!ENTITY")?.is_match(&text),
            "OPML entities and document types are not allowed"
        );
        let doc = roxmltree::Document::parse(&text)?;
        format = "opml".into();
        for (i, node) in doc
            .descendants()
            .filter(|n| {
                n.has_tag_name("outline")
                    && (n.attribute("xmlUrl").is_some() || n.attribute("xmlurl").is_some())
            })
            .enumerate()
        {
            ensure!(i < MAX_ROWS, "Import exceeds 5,000 records");
            accept(
                i + 1,
                (|| {
                    let url = normalize_url(
                        node.attribute("xmlUrl")
                            .or(node.attribute("xmlurl"))
                            .unwrap_or_default(),
                    )?
                    .to_string();
                    let mut folders: Vec<_> = node
                        .ancestors()
                        .skip(1)
                        .filter(|n| n.has_tag_name("outline"))
                        .filter_map(|n| n.attribute("title").or(n.attribute("text")))
                        .collect();
                    ensure!(folders.len() <= 32, "OPML folder nesting exceeds 32 levels");
                    folders.reverse();
                    Ok(Item::Feed(FeedItem {
                        key: url.clone(),
                        url,
                        title: node
                            .attribute("title")
                            .or(node.attribute("text"))
                            .unwrap_or_default()
                            .into(),
                        active: true,
                        metadata: json!({"folders":folders}),
                    }))
                })(),
            );
        }
    } else if text
        .to_ascii_lowercase()
        .contains("netscape-bookmark-file-1")
        || text.to_ascii_lowercase().contains("<dl")
    {
        format = "bookmarks".into();
        let doc = scraper::Html::parse_document(&text);
        for (i, a) in doc
            .select(&scraper::Selector::parse("a[href]").expect("static selector"))
            .enumerate()
        {
            ensure!(i < MAX_ROWS, "Import exceeds 5,000 records");
            accept(
                i + 1,
                article(
                    json!({"url":a.value().attr("href"),"title":a.text().collect::<String>(),"add_date":a.value().attr("add_date")}),
                    &format,
                    &files,
                ),
            );
        }
    } else {
        if text.to_ascii_lowercase().contains("in queue") || filename.contains("_matter_history") {
            format = "matter".into();
        }
        let mut reader = csv::ReaderBuilder::new()
            .flexible(false)
            .from_reader(text.as_bytes());
        let headers = reader.headers()?.clone();
        for (i, row) in reader.records().enumerate() {
            ensure!(i < MAX_ROWS, "Import exceeds 5,000 records");
            accept(
                i + 1,
                (|| {
                    let row = row?;
                    let obj: serde_json::Map<String, Value> = headers
                        .iter()
                        .zip(row.iter())
                        .map(|(k, v)| (k.into(), Value::String(v.into())))
                        .collect();
                    article(Value::Object(obj), &format, &files)
                })(),
            );
        }
    }
    ensure!(
        items.len() + problems.len() <= MAX_ROWS,
        "Import exceeds 5,000 records"
    );
    let mut seen = HashSet::new();
    let duplicate_rows = items.iter().filter(|i| !seen.insert(i.identity())).count();
    Ok(Preview {
        format,
        items,
        problems,
        warnings,
        duplicate_rows,
    })
}
pub fn stage(c: &mut Connection, path: &Path) -> Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_UPLOAD as u64,
        "Import must be a regular file under 20 MB"
    );
    let bytes = std::fs::read(path)?;
    let filename = path
        .file_name()
        .context("Missing import filename")?
        .to_string_lossy();
    let mut p = preview(&bytes, &filename)?;
    if crate::db_compat::is_lean(c)? {
        for item in &mut p.items {
            if let Item::Article(a) = item {
                a.html.clear();
            }
        }
        p.warnings.push("Article metadata and previews are kept. Full text and images are fetched on demand and may expire from the temporary cache.".into());
    }
    let fingerprint = hash(&bytes);
    if let Some(id) = c
        .query_row(
            "SELECT id FROM import_runs WHERE user_id=?1 AND fingerprint=?2",
            params![OWNER, fingerprint],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let staged = p
        .items
        .iter()
        .try_fold(0u64, |total, item| -> Result<u64> {
            Ok(total.saturating_add(serde_json::to_vec(item)?.len() as u64))
        })?;
    let _reservation = library::reserve_write(c, staged.saturating_mul(3).saturating_add(262_144))?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM import_runs WHERE user_id=?1 AND fingerprint=?2",
            params![OWNER, fingerprint],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let active:u32=tx.query_row("SELECT count(*) FROM import_runs WHERE user_id=?1 AND state IN ('preview','queued','running')",[OWNER],|r|r.get(0))?;
    ensure!(active < 2, "Finish or remove an existing import first");
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO import_runs(id,user_id,fingerprint,filename,format,total,invalid,duplicate_rows,warnings,validation_errors) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,OWNER,fingerprint,filename.chars().take(240).collect::<String>(),p.format,p.items.len() as u32,p.problems.len() as u32,p.duplicate_rows as u32,serde_json::to_string(&p.warnings)?,serde_json::to_string(&p.problems)?])?;
    for (i, item) in p.items.iter().enumerate() {
        tx.execute(
            "INSERT INTO import_rows(user_id,run_id,position,item) VALUES(?1,?2,?3,?4)",
            params![OWNER, id, (i + 1) as u32, serde_json::to_string(item)?],
        )?;
    }
    tx.commit()?;
    Ok(id)
}
pub fn list(c: &Connection) -> Result<Vec<Progress>> {
    let sql = "SELECT id,filename,format,state,total,invalid,(SELECT count(*) FROM import_rows i WHERE i.run_id=r.id AND state='pending'),(SELECT count(*) FROM import_rows i WHERE i.run_id=r.id AND state='added'),(SELECT count(*) FROM import_rows i WHERE i.run_id=r.id AND state='duplicate'),(SELECT count(*) FROM import_rows i WHERE i.run_id=r.id AND state='failed'),error,warnings,validation_errors FROM import_runs r WHERE user_id=?1 ORDER BY created_at DESC LIMIT 20";
    let rows = c
        .prepare(sql)?
        .query_map([OWNER], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u32>(4)?,
                r.get::<_, u32>(5)?,
                r.get::<_, u32>(6)?,
                r.get::<_, u32>(7)?,
                r.get::<_, u32>(8)?,
                r.get::<_, u32>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, String>(11)?,
                r.get::<_, String>(12)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|r| {
            Ok(Progress {
                id: r.0.clone(),
                filename: r.1,
                format: r.2,
                state: r.3,
                total: r.4,
                invalid: r.5,
                pending: r.6,
                added: r.7,
                duplicates: r.8,
                failed: r.9,
                error: r.10,
                warnings: serde_json::from_str(&r.11)?,
                validation_errors: serde_json::from_str(&r.12)?,
                row_errors:c.prepare("SELECT position,error FROM import_rows WHERE user_id=?1 AND run_id=?2 AND state='failed' ORDER BY position LIMIT 50")?.query_map(params![OWNER,&r.0],|row|Ok(Problem{row:row.get::<_,u32>(0)? as usize,message:row.get(1)?}))?.collect::<rusqlite::Result<_>>()?,
                preview_rows:c.prepare("SELECT item FROM import_rows WHERE user_id=?1 AND run_id=?2 ORDER BY position LIMIT 20")?.query_map(params![OWNER,&r.0],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter_map(|raw|serde_json::from_str(&raw).ok()).collect(),
            })
        })
        .collect()
}
pub fn start(c: &mut Connection, id: &str, retry: bool) -> Result<()> {
    // Claim the SQLite writer before reading run state or estimating quota.
    // A deferred WAL snapshot cannot be upgraded after intake commits a write.
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (state, total): (String, u32) = tx.query_row(
        "SELECT state,total FROM import_runs WHERE user_id=?1 AND id=?2",
        params![OWNER, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if matches!(state.as_str(), "queued" | "running") {
        return Ok(());
    }
    ensure!(total > 0, "No valid records to import");
    ensure!(
        state == "preview" || (retry && state == "failed"),
        "This import cannot be restarted"
    );
    let _reservation = library::reserve_write(&tx, mutation_bytes(&tx, id, retry, false)?)?;
    if retry {
        tx.execute("UPDATE import_rows SET state='pending',error=NULL WHERE user_id=?1 AND run_id=?2 AND state='failed'",params![OWNER,id])?;
    }
    tx.execute("UPDATE import_runs SET state='queued',attempts=0,error=NULL,lease_token=NULL,lease_until=NULL,updated_at=now() WHERE user_id=?1 AND id=?2",params![OWNER,id])?;
    tx.commit()?;
    Ok(())
}
pub fn remove(c: &Connection, id: &str) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
    let _reservation = library::reserve_write(&tx, mutation_bytes(&tx, id, false, true)?)?;
    ensure!(
        tx.execute(
            "DELETE FROM import_runs WHERE user_id=?1 AND id=?2 AND state<>'running'",
            params![OWNER, id]
        )? == 1,
        "Wait for the current import batch to finish"
    );
    tx.commit()?;
    let _ = crate::db_compat::reclaim(c);
    Ok(())
}

/// Account for the rows this atomic lifecycle change can dirty, rather than
/// claiming the complete library for a small import. A removal cannot grow its
/// database; retry can rebalance pages up to the configured database ceiling.
fn mutation_bytes(c: &Connection, id: &str, retry: bool, removal: bool) -> Result<u64> {
    let (rows, payload): (i64, i64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(item AS BLOB))+coalesce(length(CAST(error AS BLOB)),0)+coalesce(length(CAST(target_id AS BLOB)),0)+length(user_id)+length(run_id)+length(state)+64),0) FROM import_rows WHERE user_id=?1 AND run_id=?2 AND (?3 OR (?4 AND state='failed'))",
        params![OWNER, id, removal, retry],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let run_bytes: i64 = c.query_row(
        "SELECT coalesce(length(CAST(warnings AS BLOB)),0)+coalesce(length(CAST(validation_errors AS BLOB)),0)+coalesce(length(CAST(error AS BLOB)),0)+length(CAST(filename AS BLOB))+length(fingerprint)+length(id)+256 FROM import_runs WHERE user_id=?1 AND id=?2",
        params![OWNER, id],
        |r| r.get(0),
    )?;
    let page_size = u64::try_from(c.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0))?)?;
    let page_limit = u64::try_from(c.query_row(
        if removal {
            "PRAGMA page_count"
        } else {
            "PRAGMA max_page_count"
        },
        [],
        |r| r.get::<_, i64>(0),
    )?)?;
    // Half-full B-trees, row keys/indexes, freelist changes, WAL frame headers,
    // and a bounded set of root/parent pages are included before mutation.
    let affected = u64::try_from(payload)?
        .saturating_add(u64::try_from(run_bytes)?)
        .saturating_mul(2)
        .saturating_add(u64::try_from(rows)?.saturating_mul(256))
        .saturating_add(page_size.saturating_mul(16));
    let pages = affected.div_ceil(page_size).min(page_limit);
    Ok(pages
        .saturating_mul(page_size.saturating_add(24))
        .saturating_add(4096))
}
fn apply_item(tx: &Transaction<'_>, format: &str, item: &Item) -> Result<(String, bool)> {
    let (target, added, key, metadata) = match item {
        Item::Article(a) => {
            let old:Option<(String,u32,String,Option<String>)>=tx.query_row("SELECT id,word_count,status,deleted_at FROM articles WHERE user_id=?1 AND url=?2",params![OWNER,a.url],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let lean = crate::db_compat::is_lean(tx)?;
            let incoming = extraction::clean_html(&a.html, &a.url, false);
            let words = if incoming.is_empty() {
                a.word_count
            } else {
                extraction::word_count(&incoming)
            };
            let excerpt = if incoming.is_empty() {
                a.excerpt.chars().take(260).collect::<String>()
            } else {
                extraction::text(&incoming)
                    .chars()
                    .take(260)
                    .collect::<String>()
            };
            let html = if lean { String::new() } else { incoming };
            let id = old
                .as_ref()
                .map(|r| r.0.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let added = old.is_none();
            if old.is_none() {
                tx.execute("INSERT INTO articles(id,user_id,url,title,author,site,html,excerpt,word_count,status,saved_at,published_at,read_at,archived_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,COALESCE(?11,now()),?12,CASE WHEN ?13 THEN COALESCE(?14,?11,now()) ELSE NULL END,CASE WHEN ?15 THEN COALESCE(?16,?11,now()) ELSE NULL END)",params![id,OWNER,a.url,a.title,a.author,a.site,html,excerpt,words,if words>0 || lean {a.status.as_str()}else{"pending"},a.saved_at,a.published_at,a.read,a.read_at,a.archived,a.archived_at])?;
                if words == 0 {
                    library::enqueue(tx, "article", &id)?;
                }
            } else if let Some((_, old_words, status, deleted)) = &old
                && deleted.is_none()
                && (words > *old_words
                    || (words == *old_words
                        && words > 0
                        && a.status == CopyStatus::Saved
                        && status != "saved"))
            {
                tx.execute("UPDATE articles SET html=?3,excerpt=?4,word_count=?5,status=?6,error=NULL,title=?7,author=COALESCE(author,?8),published_at=COALESCE(published_at,?9) WHERE user_id=?1 AND id=?2",params![OWNER,id,html,excerpt,words,a.status.as_str(),a.title,a.author,a.published_at])?;
            }
            let key = format!(
                "import:{}",
                hash(format!("{format}\0{}\0{}", a.key, a.url).as_bytes())
            );
            tx.execute("INSERT INTO sources(user_id,article_id,kind,source_key,label) VALUES(?1,?2,'web',?3,?4) ON CONFLICT DO NOTHING",params![OWNER,id,key,format!("Imported from {format}")])?;
            (id, added, &a.key, &a.metadata)
        }
        Item::Feed(f) => {
            let old: Option<String> = tx
                .query_row(
                    "SELECT id FROM feeds WHERE user_id=?1 AND url=?2",
                    params![OWNER, f.url],
                    |r| r.get(0),
                )
                .optional()?;
            let added = old.is_none();
            let id = old.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            if added {
                tx.execute(
                    "INSERT INTO feeds(id,user_id,url,title,active) VALUES(?1,?2,?3,?4,?5)",
                    params![id, OWNER, f.url, f.title, f.active],
                )?;
                if f.active {
                    library::enqueue(tx, "feed", &id)?;
                }
            }
            (id, added, &f.key, &f.metadata)
        }
    };
    let (kind, url) = item.identity();
    tx.execute("INSERT INTO import_provenance(user_id,format,source_key,url,kind,target_id,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING",params![OWNER,format,key,url,kind,target,serde_json::to_string(metadata)?])?;
    Ok((target, added))
}
/// One durable 25-row batch. A crash repeats only rows that did not commit.
pub fn run_batch(c: &mut Connection) -> Result<bool> {
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let run:Option<(String,String)>=tx.query_row("SELECT id,format FROM import_runs WHERE user_id=?1 AND (state='queued' OR (state='running' AND lease_until<now())) ORDER BY created_at LIMIT 1",[OWNER],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((id, format)) = run else {
        return Ok(false);
    };
    let token = uuid::Uuid::new_v4().to_string();
    tx.execute("UPDATE import_runs SET state='running',lease_token=?3,lease_until=date_add(now(),2,'minutes'),attempts=attempts+1,updated_at=now() WHERE user_id=?1 AND id=?2",params![OWNER,id,token])?;
    tx.commit()?;
    let rows=c.prepare("SELECT position,item FROM import_rows WHERE user_id=?1 AND run_id=?2 AND state='pending' ORDER BY position LIMIT 25")?.query_map(params![OWNER,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (pos, raw) in rows {
        let _reservation = match library::reserve_write(c, raw.len() as u64 * 3 + 131_072) {
            Ok(reservation) => reservation,
            Err(error) => {
                // Successful rows already committed. Release only our lease so
                // the user can free space and retry pending rows immediately.
                c.execute("UPDATE import_runs SET state='failed',error=?4,lease_token=NULL,lease_until=NULL,updated_at=now() WHERE user_id=?1 AND id=?2 AND lease_token=?3",params![OWNER,id,token,error.to_string().chars().take(500).collect::<String>()])?;
                return Err(error);
            }
        };
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM import_runs WHERE user_id=?1 AND id=?2 AND lease_token=?3 AND state='running' AND lease_until>now())",params![OWNER,id,token],|r|r.get(0))?;
        ensure!(valid, "Import lease lost");
        tx.execute_batch("SAVEPOINT import_row")?;
        let result = serde_json::from_str::<Item>(&raw)
            .map_err(anyhow::Error::from)
            .and_then(|item| apply_item(&tx, &format, &item));
        match result {
            Ok((target, added)) => {
                tx.execute_batch("RELEASE import_row")?;
                tx.execute("UPDATE import_rows SET state=?4,target_id=?5,error=NULL WHERE user_id=?1 AND run_id=?2 AND position=?3",params![OWNER,id,pos,if added{"added"}else{"duplicate"},target])?;
            }
            Err(e) => {
                tx.execute_batch("ROLLBACK TO import_row;RELEASE import_row")?;
                tx.execute("UPDATE import_rows SET state='failed',error=?4 WHERE user_id=?1 AND run_id=?2 AND position=?3",params![OWNER,id,pos,e.to_string()])?;
            }
        }
        tx.commit()?;
    }
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let(pending,failed):(u32,u32)=tx.query_row("SELECT count(*) FILTER(WHERE state='pending'),count(*) FILTER(WHERE state='failed') FROM import_rows WHERE user_id=?1 AND run_id=?2",params![OWNER,id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    ensure!(tx.execute("UPDATE import_runs SET state=?4,lease_token=NULL,lease_until=NULL,updated_at=now() WHERE user_id=?1 AND id=?2 AND lease_token=?3",params![OWNER,id,token,if pending>0{"queued"}else if failed>0{"failed"}else{"completed"}])?==1,"Import lease lost");
    tx.commit()?;
    Ok(true)
}

pub fn active(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM import_runs WHERE user_id=?1 AND state IN ('queued','running'))",[OWNER],|r|r.get(0))?)
}
