use crate::domain::*;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, params};

const FILTER: &str = "a.user_id=?1 AND a.deleted_at IS NULL
AND CASE WHEN ?2='archive' THEN a.archived_at IS NOT NULL ELSE a.archived_at IS NULL END
AND CASE WHEN ?3='unread' THEN a.read_at IS NULL ELSE a.read_at IS NOT NULL END
AND (?4='' OR instr(lower(a.title || ' ' || coalesce(a.author,'') || ' ' || a.url || ' ' || a.site || ' ' || a.excerpt),lower(?4))>0)
AND (?5='' OR publication_hostname(a.url)=?5)
AND (?6='all' OR (?6='saved')=EXISTS(SELECT 1 FROM sources s WHERE s.user_id=a.user_id AND s.article_id=a.id AND s.kind<>'rss'))";
const COLUMNS: &str = "a.id,a.url,a.title,a.author,a.site,a.excerpt,a.word_count,a.status,a.error,a.published_at,a.saved_at,a.read_at,a.archived_at,
EXISTS(SELECT 1 FROM sources s WHERE s.user_id=a.user_id AND s.article_id=a.id AND s.kind<>'rss'),
EXISTS(SELECT 1 FROM magazine_exclusions e WHERE e.user_id=a.user_id AND e.article_id=a.id),
(SELECT count(*) FROM magazine_articles s JOIN magazines m ON m.id=s.magazine_id WHERE s.user_id=a.user_id AND s.article_id=a.id AND m.finalized_at IS NOT NULL),
(SELECT count(*) FROM magazine_articles s JOIN magazines m ON m.id=s.magazine_id WHERE s.user_id=a.user_id AND s.article_id=a.id AND m.finalized_at IS NULL),a.logo_url";
/// Reserve write amplification before a durable mutation. In-memory synthetic
/// databases have no managed disk files; file-backed lean databases share one owner.
pub fn reserve_write(c: &Connection, bytes: u64) -> Result<Option<crate::storage::Reservation>> {
    if crate::db_compat::is_lean(c)? && crate::storage::database_path(c)?.is_some() {
        Ok(Some(
            crate::storage::Storage::for_database(c)?.reserve("database-write", bytes)?,
        ))
    } else {
        Ok(None)
    }
}
fn invalidate_cache(c: &Connection, id: &ArticleId) -> Result<()> {
    if crate::db_compat::is_lean(c)? && crate::storage::database_path(c)?.is_some() {
        crate::storage::Storage::for_database(c)?.invalidate(&id.0)?;
    }
    Ok(())
}
fn article_row(r: &Row<'_>) -> rusqlite::Result<Article> {
    let status: String = r.get(7)?;
    let status = CopyStatus::parse(&status).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, e.into())
    })?;
    Ok(Article {
        id: ArticleId(r.get(0)?),
        url: r.get(1)?,
        title: r.get(2)?,
        author: r.get(3)?,
        site: r.get(4)?,
        excerpt: r.get(5)?,
        word_count: r.get(6)?,
        status,
        error: r.get(8)?,
        published_at: r.get(9)?,
        saved_at: r.get(10)?,
        read_at: r.get(11)?,
        archived_at: r.get(12)?,
        deliberate: r.get(13)?,
        excluded: r.get(14)?,
        printed: r.get(15)?,
        drafted: r.get(16)?,
        logo_url: r.get(17)?,
    })
}
fn query_values(q: &Query) -> [String; 6] {
    [
        OWNER.into(),
        if q.scope == Scope::Archive {
            "archive"
        } else {
            "active"
        }
        .into(),
        if q.read == ReadFilter::Read {
            "read"
        } else {
            "unread"
        }
        .into(),
        q.search.clone(),
        q.publication.clone(),
        match q.origin {
            Origin::All => "all",
            Origin::Saved => "saved",
            Origin::Rss => "discovered",
        }
        .into(),
    ]
}
pub fn articles(c: &Connection, q: &Query, offset: usize) -> Result<Vec<Article>> {
    ensure!(
        q.search.len() <= 300 && q.publication.len() <= 500 && offset <= 1_000_000,
        "Invalid article query"
    );
    let sql = format!(
        "SELECT {COLUMNS} FROM articles a WHERE {FILTER} ORDER BY {} {} NULLS LAST,a.saved_at DESC,a.id LIMIT 60 OFFSET ?7",
        q.sort.sql(),
        if q.ascending { "ASC" } else { "DESC" }
    );
    let v = query_values(q);
    Ok(c.prepare(&sql)?
        .query_map(
            params![v[0], v[1], v[2], v[3], v[4], v[5], offset as i64],
            article_row,
        )?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn publications(c: &Connection, q: &Query) -> Result<Vec<(String, u32)>> {
    let mut q = q.clone();
    q.publication.clear();
    let v = query_values(&q);
    Ok(c.prepare(&format!("SELECT publication_hostname(a.url),count(*) FROM articles a WHERE {FILTER} GROUP BY publication_hostname(a.url) ORDER BY publication_hostname(a.url)"))?.query_map(params![v[0],v[1],v[2],v[3],v[4],v[5]],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
}
pub fn matching_count(c: &Connection, q: &Query) -> Result<u32> {
    let v = query_values(q);
    Ok(c.query_row(
        &format!("SELECT count(*) FROM articles a WHERE {FILTER}"),
        params![v[0], v[1], v[2], v[3], v[4], v[5]],
        |r| r.get(0),
    )?)
}
pub fn counts(c: &Connection) -> Result<Counts> {
    Ok(c.query_row("SELECT count(*),count(*) FILTER(WHERE archived_at IS NULL),count(*) FILTER(WHERE archived_at IS NULL AND read_at IS NULL),count(*) FILTER(WHERE archived_at IS NOT NULL) FROM articles WHERE user_id=?1 AND deleted_at IS NULL",[OWNER],|r|Ok(Counts{all:r.get(0)?,inbox:r.get(1)?,unread:r.get(2)?,archive:r.get(3)?}))?)
}
pub fn metadata(c: &Connection, id: &ArticleId) -> Result<Article> {
    Ok(c.query_row(&format!("SELECT {COLUMNS} FROM articles a WHERE a.user_id=?1 AND a.id=?2 AND a.deleted_at IS NULL"),params![OWNER,id.0],article_row)?)
}
pub fn body(c: &Connection, id: &ArticleId) -> Result<ArticleBody> {
    let article = metadata(c, id)?;
    let html = c.query_row(
        "SELECT html FROM articles WHERE user_id=?1 AND id=?2",
        params![OWNER, id.0],
        |r| r.get(0),
    )?;
    let media = c
        .prepare("SELECT id,body FROM article_media WHERE user_id=?1 AND article_id=?2")?
        .query_map(params![OWNER, id.0], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ArticleBody {
        article,
        html,
        media,
    })
}
pub fn enqueue(c: &Connection, kind: &str, id: &str) -> Result<()> {
    let _reservation = reserve_write(c, 65_536)?;
    ensure!(matches!(kind, "article" | "feed"), "Invalid job kind");
    c.execute("INSERT INTO jobs(id,user_id,kind,target_id) VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,kind,target_id) DO UPDATE SET state='queued',attempts=0,run_at=now(),error=NULL WHERE jobs.state IN ('done','failed')",params![uuid::Uuid::new_v4().to_string(),OWNER,kind,id])?;
    Ok(())
}
pub fn save(
    c: &mut Connection,
    input: &str,
    kind: ArrivalKind,
    key: &str,
    label: &str,
    title: Option<&str>,
    published: Option<&str>,
) -> Result<(ArticleId, bool)> {
    let _reservation = reserve_write(c, 131_072)?;
    let tx = c.transaction()?;
    let result = save_in_transaction(&tx, input, kind, key, label, title, published)?;
    tx.commit()?;
    Ok(result)
}
pub fn save_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    input: &str,
    kind: ArrivalKind,
    key: &str,
    label: &str,
    title: Option<&str>,
    published: Option<&str>,
) -> Result<(ArticleId, bool)> {
    ensure!(
        key.len() <= 8192
            && label.len() <= 4000
            && title.is_none_or(|v| v.len() <= 4000)
            && published.is_none_or(|v| v.len() <= 100),
        "Article metadata exceeds its bound"
    );
    let url = crate::network::normalize_url(input)?;
    let host = url.host_str().unwrap_or_default();
    let id = ArticleId::new();
    let inserted=tx.execute("INSERT INTO articles(id,user_id,url,title,site,published_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(user_id,url) DO NOTHING",params![id.0,OWNER,url.as_str(),title.filter(|s|!s.trim().is_empty()).unwrap_or(host),host,published])?>0;
    let id = ArticleId(tx.query_row(
        "SELECT id FROM articles WHERE user_id=?1 AND url=?2",
        params![OWNER, url.as_str()],
        |r| r.get(0),
    )?);
    let restored = if kind == ArrivalKind::Web || kind == ArrivalKind::WhatsApp {
        tx.execute("UPDATE articles SET deleted_at=NULL,read_at=NULL,archived_at=NULL,status='pending',saved_at=now() WHERE user_id=?1 AND id=?2 AND deleted_at IS NOT NULL AND (?3='web' OR NOT EXISTS(SELECT 1 FROM sources WHERE user_id=?1 AND kind=?3 AND source_key=?4))",params![OWNER,id.0,kind.as_str(),key])?>0
    } else {
        false
    };
    tx.execute("INSERT INTO sources(user_id,article_id,kind,source_key,label) VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",params![OWNER,id.0,kind.as_str(),key,label])?;
    if kind != ArrivalKind::Rss || !crate::db_compat::is_lean(tx)? {
        tx.execute("INSERT INTO jobs(id,user_id,kind,target_id) SELECT ?1,?2,'article',?3 FROM articles WHERE user_id=?2 AND id=?3 AND deleted_at IS NULL ON CONFLICT DO NOTHING",params![uuid::Uuid::new_v4().to_string(),OWNER,id.0])?;
    }
    Ok((id, inserted || restored))
}
pub fn update(
    c: &Connection,
    id: &ArticleId,
    read: Option<bool>,
    archived: Option<bool>,
) -> Result<()> {
    let _reservation = reserve_write(c, 131_072)?;
    if archived == Some(true) {
        invalidate_cache(c, id)?;
    }
    let tx = c.unchecked_transaction()?;
    ensure!(tx.execute("UPDATE articles SET read_at=CASE WHEN ?4=1 THEN COALESCE(read_at,now()) WHEN ?3 IS NULL THEN read_at WHEN ?3=1 THEN COALESCE(read_at,now()) ELSE NULL END, archived_at=CASE WHEN ?4 IS NULL THEN archived_at WHEN ?4=1 THEN COALESCE(archived_at,now()) ELSE NULL END WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL",params![OWNER,id.0,read,archived])?==1,"Article not found");
    if archived == Some(true) && crate::db_compat::is_lean(&tx)? {
        tx.execute(
            "UPDATE articles SET html='',logo_url=NULL WHERE user_id=?1 AND id=?2",
            params![OWNER, id.0],
        )?;
        tx.execute("DELETE FROM article_media AS media WHERE user_id=?1 AND article_id=?2 AND NOT EXISTS(SELECT 1 FROM magazine_articles s WHERE s.user_id=?1 AND instr(s.html,media.id)>0)",params![OWNER,id.0])?;
    }
    tx.commit()?;
    Ok(())
}
pub fn delete(c: &mut Connection, ids: &[ArticleId]) -> Result<usize> {
    let _reservation = reserve_write(c, 131_072 + ids.len() as u64 * 8192)?;
    for id in ids {
        invalidate_cache(c, id)?;
    }
    let tx = c.transaction()?;
    let lean = crate::db_compat::is_lean(&tx)?;
    let mut count = 0;
    for id in ids {
        let sql = if lean {
            "UPDATE articles SET deleted_at=now(),html='',logo_url=NULL WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL"
        } else {
            "UPDATE articles SET deleted_at=now(),html='',excerpt='',word_count=0,logo_url=NULL,error=NULL,status='pending' WHERE user_id=?1 AND id=?2 AND deleted_at IS NULL"
        };
        count += tx.execute(sql, params![OWNER, id.0])?;
        tx.execute(
            "DELETE FROM jobs WHERE user_id=?1 AND kind='article' AND target_id=?2",
            params![OWNER, id.0],
        )?;
        tx.execute("DELETE FROM article_media AS media WHERE user_id=?1 AND article_id=?2 AND NOT EXISTS(SELECT 1 FROM magazine_articles s WHERE s.user_id=?1 AND instr(s.html,media.id)>0)",params![OWNER,id.0])?;
    }
    tx.commit()?;
    Ok(count)
}
pub fn delete_matching(c: &mut Connection, q: &Query, expected: u32) -> Result<usize> {
    let _reservation = reserve_write(c, 131_072 + expected as u64 * 8192)?;
    let tx = c.transaction()?;
    let v = query_values(q);
    let ids = tx
        .prepare(&format!(
            "SELECT a.id FROM articles a WHERE {FILTER} ORDER BY a.id"
        ))?
        .query_map(params![v[0], v[1], v[2], v[3], v[4], v[5]], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        ids.len() == expected as usize,
        "The library changed. Review the matching article count again."
    );
    for id in &ids {
        invalidate_cache(&tx, &ArticleId(id.clone()))?;
        let sql = if crate::db_compat::is_lean(&tx)? {
            "UPDATE articles SET deleted_at=now(),html='',logo_url=NULL WHERE user_id=?1 AND id=?2"
        } else {
            "UPDATE articles SET deleted_at=now(),html='',excerpt='',word_count=0,logo_url=NULL,error=NULL,status='pending' WHERE user_id=?1 AND id=?2"
        };
        tx.execute(sql, params![OWNER, id])?;
        tx.execute(
            "DELETE FROM jobs WHERE user_id=?1 AND kind='article' AND target_id=?2",
            params![OWNER, id],
        )?;
        tx.execute("DELETE FROM article_media AS media WHERE user_id=?1 AND article_id=?2 AND NOT EXISTS(SELECT 1 FROM magazine_articles s WHERE s.user_id=?1 AND instr(s.html,media.id)>0)",params![OWNER,id])?;
    }
    tx.commit()?;
    Ok(ids.len())
}
pub fn refetch_all(c: &mut Connection) -> Result<usize> {
    let _reservation = reserve_write(c, 1_000_000)?;
    let tx = c.transaction()?;
    let ids=tx.prepare("SELECT a.id FROM articles a WHERE user_id=?1 AND deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.user_id=a.user_id AND j.target_id=a.id AND j.kind='article' AND j.state IN ('queued','running')) ORDER BY saved_at DESC,id LIMIT 1500")?.query_map([OWNER],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for id in &ids {
        enqueue(&tx, "article", id)?;
    }
    tx.commit()?;
    Ok(ids.len())
}
pub fn feeds(c: &Connection) -> Result<Vec<Feed>> {
    Ok(c.prepare("SELECT f.id,f.url,f.title,f.active,f.last_polled_at,f.error,j.state FROM feeds f LEFT JOIN jobs j ON j.user_id=f.user_id AND j.kind='feed' AND j.target_id=f.id WHERE f.user_id=?1 ORDER BY f.created_at DESC,f.id")?.query_map([OWNER],|r|Ok(Feed{id:FeedId(r.get(0)?),url:r.get(1)?,title:r.get(2)?,active:r.get(3)?,last_polled_at:r.get(4)?,error:r.get(5)?,activity:match r.get::<_,Option<String>>(6)?.as_deref(){Some("queued")=>FeedActivity::Queued,Some("running")=>FeedActivity::Running,_=>FeedActivity::Idle}}))?.collect::<rusqlite::Result<_>>()?)
}
pub fn add_feed(c: &mut Connection, input: &str) -> Result<FeedId> {
    let _reservation = reserve_write(c, 131_072)?;
    let url = crate::network::normalize_url(input)?;
    let tx = c.transaction()?;
    let id:FeedId=FeedId(tx.query_row("INSERT INTO feeds(id,user_id,url,title) VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,url) DO UPDATE SET active=1 RETURNING id",params![FeedId::new().0,OWNER,url.as_str(),url.host_str().unwrap_or_default()],|r|r.get(0))?);
    enqueue(&tx, "feed", &id.0)?;
    tx.commit()?;
    Ok(id)
}
pub fn update_feed(c: &mut Connection, id: &FeedId, action: FeedAction) -> Result<()> {
    let _reservation = reserve_write(c, 131_072)?;
    let tx = c.transaction()?;
    ensure!(
        tx.query_row(
            "SELECT 1 FROM feeds WHERE user_id=?1 AND id=?2",
            params![OWNER, id.0],
            |r| r.get::<_, i32>(0)
        )
        .optional()?
        .is_some(),
        "Feed not found"
    );
    match action {
        FeedAction::Pause | FeedAction::Resume => {
            tx.execute(
                "UPDATE feeds SET active=?3,next_poll_at=now() WHERE user_id=?1 AND id=?2",
                params![OWNER, id.0, matches!(action, FeedAction::Resume)],
            )?;
            if matches!(action, FeedAction::Resume) {
                enqueue(&tx, "feed", &id.0)?;
            }
        }
        FeedAction::Sync => enqueue(&tx, "feed", &id.0)?,
        FeedAction::Remove => {
            if crate::db_compat::is_lean(&tx)? {
                tx.execute(
                    "DELETE FROM feed_captures WHERE user_id=?1 AND feed_id=?2",
                    params![OWNER, id.0],
                )?;
            }
            tx.execute(
                "DELETE FROM feeds WHERE user_id=?1 AND id=?2",
                params![OWNER, id.0],
            )?;
            tx.execute(
                "DELETE FROM jobs WHERE user_id=?1 AND kind='feed' AND target_id=?2",
                params![OWNER, id.0],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}
pub fn magazines(c: &Connection) -> Result<Vec<Magazine>> {
    let viewer_receipts: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='native_viewer_editions')", [], |row| row.get(0))?;
    let (pages, receipt_join) = if viewer_receipts {
        (
            "COALESCE(p.page_count,v.page_count)",
            "LEFT JOIN native_viewer_editions v ON v.magazine_id=m.id",
        )
    } else {
        ("p.page_count", "")
    };
    let sql = format!(
        "SELECT m.id,m.title,m.subtitle,m.created_at,m.finalized_at,row_number() OVER(ORDER BY m.created_at,m.id),COALESCE(p.paper_size,m.paper_size),{pages} FROM magazines m LEFT JOIN magazine_pdfs p ON p.user_id=m.user_id AND p.magazine_id=m.id {receipt_join} WHERE m.user_id=?1 ORDER BY m.created_at DESC,m.id DESC"
    );
    Ok(c.prepare(&sql)?
        .query_map([OWNER], |row| {
            let paper: String = row.get(6)?;
            let paper_size = crate::magazine::PaperSize::parse(&paper).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    error.into(),
                )
            })?;
            Ok(Magazine {
                id: MagazineId(row.get(0)?),
                title: row.get(1)?,
                subtitle: row.get(2)?,
                created_at: row.get(3)?,
                finalized_at: row.get(4)?,
                issue_number: row.get(5)?,
                paper_size,
                page_count: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn stored_pdf(c: &Connection, id: &MagazineId) -> Result<Vec<u8>> {
    Ok(c.query_row(
        "SELECT body FROM magazine_pdfs WHERE user_id=?1 AND magazine_id=?2",
        params![OWNER, id.0],
        |r| r.get(0),
    )?)
}
pub fn logos(c: &Connection, articles: &[Article]) -> Result<Vec<(String, Vec<u8>)>> {
    if crate::db_compat::is_lean(c)? {
        return crate::icons::cached_logos(c, articles);
    }
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for article in articles {
        if let Some(url) = &article.logo_url
            && let Some(id) = url.strip_prefix("/api/media/")
                && seen.insert(id)
                    && let Some(bytes)=c.query_row("SELECT body FROM article_media WHERE user_id=?1 AND id=?2 AND length(body)<=524288",params![OWNER,id],|r|r.get(0)).optional()?{out.push((url.clone(),bytes));}
    }
    Ok(out)
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::db_compat::register(&connection).unwrap();
        crate::db_compat::initialize(&connection).unwrap();
        connection
    }

    #[test]
    fn feed_activity_comes_from_its_own_job_and_preserves_subscription_state() {
        let mut connection = connection();
        let id = add_feed(&mut connection, "https://example.org/feed").unwrap();
        connection
            .execute(
                "UPDATE feeds SET active=0,error='Previous failure' WHERE id=?1",
                [&id.0],
            )
            .unwrap();
        for (state, expected) in [
            ("queued", FeedActivity::Queued),
            ("running", FeedActivity::Running),
            ("done", FeedActivity::Idle),
            ("failed", FeedActivity::Idle),
        ] {
            connection
                .execute(
                    "UPDATE jobs SET state=?1 WHERE kind='feed' AND target_id=?2",
                    params![state, id.0],
                )
                .unwrap();
            let feed = feeds(&connection).unwrap().remove(0);
            assert_eq!(feed.activity, expected);
            assert!(!feed.active);
            assert_eq!(feed.error.as_deref(), Some("Previous failure"));
        }
        connection
            .execute(
                "DELETE FROM jobs WHERE kind='feed' AND target_id=?1",
                [&id.0],
            )
            .unwrap();
        enqueue(&connection, "article", &id.0).unwrap();
        assert_eq!(
            feeds(&connection).unwrap()[0].activity,
            FeedActivity::Idle,
            "an unrelated article job must not animate a feed"
        );
    }

    #[test]
    fn magazine_summary_uses_actual_pdf_geometry_and_retained_page_receipts() {
        let connection = connection();
        connection.execute("INSERT INTO magazines(id,user_id,title,paper_size) VALUES('summary',?1,'Synthetic issue','A5')", [OWNER]).unwrap();
        let draft = magazines(&connection).unwrap().remove(0);
        assert_eq!(draft.paper_size, crate::magazine::PaperSize::A5);
        assert_eq!(draft.page_count, None);
        connection.execute("INSERT INTO magazine_pdfs(user_id,magazine_id,body,sha256,page_count,paper_size,renderer_version,verification) VALUES(?1,'summary',X'00',?2,12,'Letter','synthetic','{}')", params![OWNER,"a".repeat(64)]).unwrap();
        let exact_pdf = magazines(&connection).unwrap().remove(0);
        assert_eq!(exact_pdf.paper_size, crate::magazine::PaperSize::Letter);
        assert_eq!(exact_pdf.page_count, Some(12));
        connection.execute_batch("CREATE TABLE native_viewer_editions(magazine_id TEXT PRIMARY KEY,page_count INTEGER NOT NULL)").unwrap();
        connection
            .execute(
                "INSERT INTO native_viewer_editions VALUES('summary',12)",
                [],
            )
            .unwrap();
        connection
            .execute("DELETE FROM magazine_pdfs WHERE magazine_id='summary'", [])
            .unwrap();
        let removed_pdf = magazines(&connection).unwrap().remove(0);
        assert_eq!(removed_pdf.paper_size, crate::magazine::PaperSize::A5);
        assert_eq!(removed_pdf.page_count, Some(12));
    }
}
