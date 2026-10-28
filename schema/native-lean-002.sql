-- The installed reader owns metadata and bounded temporary article copies.
CREATE TABLE IF NOT EXISTS native_storage_policy (
  id INTEGER PRIMARY KEY CHECK(id=1),
  version INTEGER NOT NULL CHECK(version=2),
  limit_bytes INTEGER NOT NULL CHECK(limit_bytes=100000000),
  cache_bytes INTEGER NOT NULL CHECK(cache_bytes=20000000),
  applied_at TEXT NOT NULL DEFAULT(now())
);
INSERT OR IGNORE INTO native_storage_policy(id,version,limit_bytes,cache_bytes)
VALUES(1,2,100000000,20000000);
CREATE TABLE IF NOT EXISTS native_viewer_editions (
  magazine_id TEXT PRIMARY KEY REFERENCES magazines(id) ON DELETE CASCADE,
  sha256 TEXT NOT NULL CHECK(length(sha256)=64),
  page_count INTEGER NOT NULL,
  provenance_sha256 TEXT NOT NULL CHECK(length(provenance_sha256)=64)
);
CREATE TRIGGER IF NOT EXISTS immutable_native_viewer_update BEFORE UPDATE ON native_viewer_editions
BEGIN SELECT RAISE(ABORT,'Finalized viewer provenance is immutable'); END;
CREATE TRIGGER IF NOT EXISTS immutable_native_viewer_delete BEFORE DELETE ON native_viewer_editions
WHEN EXISTS(SELECT 1 FROM magazines WHERE id=OLD.magazine_id AND finalized_at IS NOT NULL)
BEGIN SELECT RAISE(ABORT,'Finalized viewer provenance is immutable'); END;
-- Durable articles contain no full-copy payload. Cache availability is separate
-- from the last extraction quality and never changes read/archive state.
CREATE TRIGGER IF NOT EXISTS lean_article_insert BEFORE INSERT ON articles WHEN NEW.html<>''
BEGIN SELECT RAISE(ABORT,'Lean articles retain metadata only'); END;
CREATE TRIGGER IF NOT EXISTS lean_article_update BEFORE UPDATE OF html ON articles WHEN NEW.html<>''
BEGIN SELECT RAISE(ABORT,'Lean articles retain metadata only'); END;
CREATE TRIGGER IF NOT EXISTS lean_article_media_insert BEFORE INSERT ON article_media
BEGIN SELECT RAISE(ABORT,'Article images belong in the bounded cache'); END;
INSERT OR IGNORE INTO native_schema_migrations(version) VALUES(2);
