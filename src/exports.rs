//! Streaming portable export. Credential and transport tables are never selected.
use crate::domain::OWNER;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, types::ValueRef};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o600)
}
struct ExportFile {
    file: File,
    limit: Option<u64>,
}
impl Write for ExportFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.limit {
            let position = self.file.stream_position()?;
            if position.saturating_add(bytes.len() as u64) > limit {
                return Err(std::io::Error::other(
                    "Export exceeded its reserved storage budget",
                ));
            }
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for ExportFile {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(from)
    }
}
fn entry(zip: &mut ZipWriter<ExportFile>, name: &str, bytes: &[u8]) -> Result<()> {
    zip.start_file(name, options())?;
    zip.write_all(bytes)?;
    Ok(())
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub fn opml(c: &Connection) -> Result<String> {
    let feeds = crate::library::feeds(c)?;
    let mut out="<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\"><head><title>Read Later subscriptions</title></head><body>".to_owned();
    for f in feeds {
        out.push_str(&format!(
            "<outline type=\"rss\" text=\"{}\" title=\"{}\" xmlUrl=\"{}\"/>\n",
            xml(&f.title),
            xml(&f.title),
            xml(&f.url)
        ));
    }
    out.push_str("</body></opml>\n");
    Ok(out)
}
fn plain_row(
    row: &rusqlite::Row<'_>,
    names: &[String],
) -> rusqlite::Result<serde_json::Map<String, Value>> {
    let mut object = serde_json::Map::new();
    for (i, name) in names.iter().enumerate() {
        let v = match row.get_ref(i)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(n) => json!(n),
            ValueRef::Real(n) => json!(n),
            ValueRef::Text(b) => {
                let s = String::from_utf8_lossy(b).into_owned();
                if matches!(
                    name.as_str(),
                    "design"
                        | "presentation"
                        | "provenance"
                        | "metadata"
                        | "request"
                        | "verification"
                ) {
                    serde_json::from_str(&s).unwrap_or(Value::String(s))
                } else {
                    Value::String(s)
                }
            }
            ValueRef::Blob(_) => Value::Null,
        };
        object.insert(name.clone(), v);
    }
    Ok(object)
}
pub fn export(c: &Connection, path: &Path) -> Result<()> {
    let executable = std::env::current_exe()?;
    reject_bundle_destination(path, &executable)?;
    ensure!(
        path.parent().is_some_and(Path::is_dir),
        "Export destination folder is missing"
    );
    ensure!(
        !path.exists(),
        "Export destination already exists. Choose a new filename."
    );
    let lean = crate::db_compat::is_lean(c)?;
    let snapshot = c.unchecked_transaction()?;
    let managed = if lean && crate::storage::database_path(c)?.is_some() {
        let storage = crate::storage::Storage::for_database(c)?;
        path.parent()
            .context("Missing export directory")?
            .canonicalize()?
            .starts_with(storage.root())
    } else {
        false
    };
    let reservation = if managed {
        let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let pages = u64::try_from(pages)?;
        let page_size = u64::try_from(page_size)?;
        crate::library::reserve_write(
            c,
            pages
                .saturating_mul(page_size)
                .saturating_mul(2)
                .saturating_add(1_048_576),
        )?
    } else {
        None
    };
    let limit = reservation.as_ref().map(crate::storage::Reservation::bytes);
    let pending = path.with_extension("zip.pending");
    ensure!(
        !pending.exists(),
        "An export already exists at this pending path"
    );
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .mode(0o600)
        .open(&pending)?;
    let result = (|| {
        let mut zip = ZipWriter::new(ExportFile { file, limit });
        let mut article_metadata = vec![];
        let mut media_metadata = vec![];
        let mut csv = csv::Writer::from_writer(Vec::new());
        csv.write_record([
            "url",
            "title",
            "author",
            "site",
            "saved_at",
            "published_at",
            "read",
            "archived",
            "read_at",
            "archived_at",
            "status",
        ])?;
        let mut statement=snapshot.prepare("SELECT id,url,title,author,site,html,excerpt,word_count,status,error,published_at,saved_at,read_at,archived_at FROM articles WHERE user_id=?1 AND deleted_at IS NULL ORDER BY id")?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut rows = statement.query([OWNER])?;
        while let Some(row) = rows.next()? {
            let mut a = plain_row(row, &names)?;
            let id = a["id"].as_str().context("Invalid article ID")?.to_owned();
            let html = a
                .remove("html")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let html_path = (!lean && !html.is_empty()).then(|| format!("articles/{id}.html"));
            if let Some(path) = &html_path {
                entry(&mut zip, path, html.as_bytes())?;
            }
            a.insert("kind".into(), json!("article"));
            a.insert("key".into(), json!(id));
            a.insert("htmlPath".into(), json!(html_path));
            a.insert(
                "bodyAvailability".into(),
                json!(if lean {
                    "temporary-cache-not-exported"
                } else if html.is_empty() {
                    "unavailable"
                } else {
                    "included"
                }),
            );
            let read = !a["read_at"].is_null();
            let archived = !a["archived_at"].is_null();
            a.insert("read".into(), json!(read));
            a.insert("archived".into(), json!(archived));
            let mut cells = vec![];
            for name in [
                "url",
                "title",
                "author",
                "site",
                "saved_at",
                "published_at",
                "read",
                "archived",
                "read_at",
                "archived_at",
                "status",
            ] {
                let v = &a[name];
                let mut s = if v.is_null() {
                    String::new()
                } else {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                };
                if s.starts_with(['=', '+', '@', '-', '\t', '\r']) {
                    s.insert(0, '\'');
                }
                cells.push(s);
            }
            csv.write_record(cells)?;
            article_metadata.push(Value::Object(a));
        }
        let mut statement =
            snapshot.prepare("SELECT * FROM article_media WHERE user_id=?1 ORDER BY id")?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut rows = statement.query([OWNER])?;
        while let Some(row) = rows.next()? {
            if lean {
                continue;
            }
            let mut m = plain_row(row, &names)?;
            let id = m["id"].as_str().context("Invalid media ID")?;
            let path = format!("media/{id}");
            let bytes: Vec<u8> = row.get("body")?;
            ensure!(
                format!("{:x}", Sha256::digest(&bytes))
                    == m["sha256"].as_str().context("Missing media hash")?,
                "Media hash mismatch"
            );
            entry(&mut zip, &path, &bytes)?;
            m.remove("body");
            m.insert("path".into(), json!(path));
            media_metadata.push(Value::Object(m));
        }
        let mut feeds = vec![];
        for f in crate::library::feeds(&snapshot)? {
            feeds.push(json!({"key":f.id.0,"url":f.url,"title":f.title,"active":f.active,"last_polled_at":f.last_polled_at,"error":f.error}));
        }
        // Available edition/provenance rows. Retired production originals and
        // disposable reader cache are intentionally absent in the lean contract.
        let mut table_manifest = serde_json::Map::new();
        for table in [
            "sources",
            "import_provenance",
            "magazines",
            "magazine_articles",
            "magazine_artwork",
            "magazine_covers",
            "magazine_cover_requests",
            "magazine_origins",
            "magazine_pdfs",
            "magazine_exclusions",
            "magazine_publication_exclusions",
            "magazine_policy_imports",
            "native_magazine_source_repairs",
            "native_article_captures",
            "native_intake_attempts",
            "native_magazine_media",
            "native_magazine_acquisitions",
            "native_magazine_jobs",
            "native_magazine_proofs",
            "native_viewer_editions",
            "native_viewer_provenance",
            "native_pdf_exports",
        ] {
            if lean && matches!(table, "native_article_captures") {
                continue;
            }
            let exists: bool = snapshot.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )?;
            if !exists {
                continue;
            }
            let edition_owned = matches!(
                table,
                "native_magazine_media"
                    | "native_magazine_acquisitions"
                    | "native_magazine_jobs"
                    | "native_magazine_proofs"
                    | "native_viewer_editions"
                    | "native_viewer_provenance"
                    | "native_pdf_exports"
            );
            let sql = if edition_owned {
                format!(
                    "SELECT * FROM {table} WHERE magazine_id IN (SELECT id FROM magazines WHERE user_id=?1) ORDER BY rowid"
                )
            } else if matches!(
                table,
                "native_magazine_source_repairs" | "native_intake_attempts"
            ) {
                format!("SELECT * FROM {table} ORDER BY rowid")
            } else {
                format!("SELECT * FROM {table} WHERE user_id=?1 ORDER BY rowid")
            };
            let mut statement = snapshot.prepare(&sql)?;
            let names: Vec<String> = statement
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut rows = if matches!(
                table,
                "native_magazine_source_repairs" | "native_intake_attempts"
            ) {
                statement.query([])?
            } else {
                statement.query([OWNER])?
            };
            let path = format!("tables/{table}.json");
            zip.start_file(&path, options())?;
            zip.write_all(b"[")?;
            let mut index = 0;
            // Each table JSON is bounded by metadata/snapshot sizes, with BLOBs separate.
            while let Some(row) = rows.next()? {
                let mut object = plain_row(row, &names)?;
                for (i, name) in names.iter().enumerate() {
                    if let ValueRef::Blob(bytes) = row.get_ref(i)? {
                        let binary_path = format!("binary/{table}/{index}-{name}");
                        object.insert(name.clone(),json!({"path":binary_path,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}));
                    }
                }
                if index > 0 {
                    zip.write_all(b",")?;
                }
                serde_json::to_writer(&mut zip, &object)?;
                index += 1;
            }
            zip.write_all(b"]")?;
            drop(rows);
            let mut rows = if matches!(
                table,
                "native_magazine_source_repairs" | "native_intake_attempts"
            ) {
                statement.query([])?
            } else {
                statement.query([OWNER])?
            };
            let mut index = 0;
            while let Some(row) = rows.next()? {
                for (i, name) in names.iter().enumerate() {
                    if let ValueRef::Blob(bytes) = row.get_ref(i)? {
                        entry(&mut zip, &format!("binary/{table}/{index}-{name}"), bytes)?;
                    }
                }
                index += 1;
            }
            table_manifest.insert(table.into(), json!({"path":path,"rows":index}));
        }
        let manifest = json!({"format":"reading-list-v1","exported_at":chrono::Utc::now().to_rfc3339(),"articles":article_metadata,"feeds":feeds,"media":media_metadata,"nativeTables":table_manifest,"retention":if lean {json!({"policy":"metadata-and-viewer-pdfs-v1","articleBodies":"on-demand; temporary cache excluded","historicalEditions":"exact viewer PDF plus edition metadata","productionOriginals":"retired; no reconstruction promised"})} else {json!({"policy":"legacy-retained-copies"})}});
        entry(
            &mut zip,
            "reading-list.json",
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        entry(&mut zip, "articles.csv", &csv.into_inner()?)?;
        entry(&mut zip, "subscriptions.opml", opml(&snapshot)?.as_bytes())?;
        let f = zip.finish()?;
        f.file.sync_all()?;
        Ok(())
    })();
    snapshot.rollback()?;
    if result.is_err() {
        let _ = std::fs::remove_file(&pending);
        return result;
    }
    std::fs::rename(&pending, path)?;
    File::open(path.parent().context("Missing destination folder")?)?.sync_all()?;
    Ok(())
}

fn reject_bundle_destination(path: &Path, executable: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("Missing export directory")?
        .canonicalize()?;
    let target = parent.join(path.file_name().context("Missing export filename")?);
    if let Some(bundle) = executable.ancestors().find(|ancestor| {
        ancestor
            .extension()
            .is_some_and(|extension| extension == "app")
    }) {
        ensure!(
            !target.starts_with(bundle.canonicalize()?),
            "Export outside the installed application bundle"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_export_rejects_bundle_and_symlink_alias_before_writing() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Reader.app");
        let resources = bundle.join("Contents/Resources");
        let exports = root.path().join("exports");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::create_dir(&exports).unwrap();
        let alias = exports.join("bundle-link");
        std::os::unix::fs::symlink(&resources, &alias).unwrap();
        let executable = bundle.join("Contents/MacOS/reader");
        for parent in [&resources, &alias] {
            let path = parent.join("export.zip");
            assert!(reject_bundle_destination(&path, &executable).is_err());
            assert!(!path.exists());
            assert!(!path.with_extension("zip.pending").exists());
        }
        reject_bundle_destination(&exports.join("export.zip"), &executable).unwrap();
    }
}
