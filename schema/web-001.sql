-- SQLite version 001 models the current reader, rather than replaying historical PostgreSQL migrations.
-- Ownership columns preserve saved relationships and immutable snapshots; they are not browser-selected accounts.

CREATE TABLE users (
  id TEXT NOT NULL,
  email TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (now()),
  disabled_at TEXT,
  library_revision INTEGER NOT NULL DEFAULT (0),
  PRIMARY KEY (id)
 );

CREATE TABLE account_deletions (
  user_id TEXT NOT NULL,
  requested_at TEXT NOT NULL DEFAULT (now()),
  state TEXT NOT NULL DEFAULT ('queued'),
  attempts INTEGER NOT NULL DEFAULT (0),
  auth_deleted BOOLEAN NOT NULL DEFAULT (0) CHECK (auth_deleted IN (0,1)),
  run_at TEXT NOT NULL DEFAULT (now()),
  lease_token TEXT,
  lease_until TEXT,
  error TEXT,
  completed_at TEXT,
  PRIMARY KEY (user_id),
  CHECK ((state IN ('queued', 'running', 'done')))
 );

CREATE TABLE article_media (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  article_id TEXT NOT NULL,
  source_url TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  content_type TEXT NOT NULL,
  body BLOB NOT NULL,
  PRIMARY KEY (id),
  FOREIGN KEY (user_id, article_id) REFERENCES articles(user_id, id) ON DELETE CASCADE,
  UNIQUE (user_id, article_id, source_url, sha256),
  CHECK (length(body) BETWEEN 1 AND 5242880)
 );

CREATE TABLE articles (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  url TEXT NOT NULL,
  title TEXT NOT NULL,
  author TEXT,
  site TEXT NOT NULL,
  html TEXT NOT NULL DEFAULT (''),
  excerpt TEXT NOT NULL DEFAULT (''),
  word_count INTEGER NOT NULL DEFAULT (0),
  status TEXT NOT NULL DEFAULT ('pending'),
  error TEXT,
  published_at TEXT,
  saved_at TEXT NOT NULL DEFAULT (now()),
  read_at TEXT,
  archived_at TEXT,
  deleted_at TEXT,
  logo_url TEXT,
  search_text TEXT GENERATED ALWAYS AS (lower(title || ' ' || coalesce(author,'') || ' ' || url || ' ' || site || ' ' || excerpt || ' ' || strip_html(html))) STORED,
  updated_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (id),
  CHECK ((status IN ('pending', 'saved', 'partial', 'failed'))),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  UNIQUE (user_id, id),
  UNIQUE (user_id, url)
 );

CREATE TABLE backup_health (
  id INTEGER NOT NULL,
  verified_at TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  CHECK ((id = 1)),
  PRIMARY KEY (id)
 );

CREATE TABLE feed_captures (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  feed_id TEXT NOT NULL,
  xml TEXT NOT NULL,
  received_at TEXT NOT NULL DEFAULT (now()),
  processed_at TEXT,
  content_hash TEXT GENERATED ALWAYS AS (md5(xml)) STORED,
  PRIMARY KEY (id),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE feeds (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  url TEXT NOT NULL,
  title TEXT NOT NULL,
  active BOOLEAN NOT NULL DEFAULT (1) CHECK (active IN (0,1)),
  last_polled_at TEXT,
  next_poll_at TEXT NOT NULL DEFAULT (now()),
  error TEXT,
  created_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (id),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  UNIQUE (user_id, id),
  UNIQUE (user_id, url)
 );

CREATE TABLE import_provenance (
  user_id TEXT NOT NULL,
  format TEXT NOT NULL,
  source_key TEXT NOT NULL,
  url TEXT NOT NULL,
  kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  metadata JSON NOT NULL CHECK (metadata IS NULL OR json_valid(metadata)),
  imported_at TEXT NOT NULL DEFAULT (now()),
  CHECK ((kind IN ('article', 'feed'))),
  PRIMARY KEY (user_id, format, kind, source_key, url),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE import_rows (
  user_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  item JSON NOT NULL CHECK (item IS NULL OR json_valid(item)),
  state TEXT NOT NULL DEFAULT ('pending'),
  error TEXT,
  target_id TEXT,
  PRIMARY KEY (run_id, "position"),
  CHECK ((state IN ('pending', 'added', 'duplicate', 'failed'))),
  FOREIGN KEY (user_id, run_id) REFERENCES import_runs(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE import_runs (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  filename TEXT NOT NULL,
  format TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT ('preview'),
  total INTEGER NOT NULL,
  invalid INTEGER NOT NULL DEFAULT (0),
  duplicate_rows INTEGER NOT NULL DEFAULT (0),
  warnings JSON NOT NULL DEFAULT ('[]') CHECK (warnings IS NULL OR json_valid(warnings)),
  validation_errors JSON NOT NULL DEFAULT ('[]') CHECK (validation_errors IS NULL OR json_valid(validation_errors)),
  attempts INTEGER NOT NULL DEFAULT (0),
  lease_token TEXT,
  lease_until TEXT,
  error TEXT,
  created_at TEXT NOT NULL DEFAULT (now()),
  updated_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (id),
  CHECK ((state IN ('preview', 'queued', 'running', 'completed', 'failed'))),
  CHECK (((total >= 0) AND (total <= 5000))),
  UNIQUE (user_id, fingerprint),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  UNIQUE (user_id, id)
 );

CREATE TABLE jobs (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT ('queued'),
  attempts INTEGER NOT NULL DEFAULT (0),
  run_at TEXT NOT NULL DEFAULT (now()),
  lease_until TEXT,
  lease_token TEXT,
  error TEXT,
  CHECK ((kind IN ('article', 'feed'))),
  PRIMARY KEY (id),
  CHECK ((state IN ('queued', 'running', 'done', 'failed'))),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  UNIQUE (user_id, kind, target_id)
 );

CREATE TABLE magazine_articles (
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  article_id TEXT NOT NULL,
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  author TEXT,
  html TEXT NOT NULL,
  word_count INTEGER NOT NULL,
  content_status TEXT NOT NULL DEFAULT ('saved'),
  content_warning TEXT,
  presentation JSON NOT NULL DEFAULT ('{}') CHECK (presentation IS NULL OR json_valid(presentation)),
  CHECK ((content_status IN ('saved', 'partial', 'pending', 'failed'))),
  UNIQUE (magazine_id, article_id),
  PRIMARY KEY (magazine_id, "position"),
  FOREIGN KEY (user_id, article_id) REFERENCES articles(user_id, id) DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE magazine_artwork (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  content_type TEXT NOT NULL,
  body BLOB NOT NULL,
  sha256 TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  label TEXT NOT NULL,
  source_url TEXT,
  created_at TEXT NOT NULL DEFAULT (now()),
  CHECK (((length(body) >= 1) AND (length(body) <= 33554432))),
  CHECK ((content_type IN ('image/png', 'image/jpeg'))),
  CHECK (((height >= 1) AND (height <= 12000))),
  PRIMARY KEY (id),
  CHECK ((length(sha256) = 64)),
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE,
  UNIQUE (user_id, magazine_id, id),
  CHECK (((width >= 1) AND (width <= 12000)))
 );

CREATE TABLE magazine_cover_requests (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  input_sha256 TEXT NOT NULL,
  request JSON NOT NULL CHECK (request IS NULL OR json_valid(request)),
  state TEXT NOT NULL DEFAULT ('waiting'),
  front_asset TEXT,
  back_asset TEXT,
  original_front BLOB,
  original_back BLOB,
  provenance JSON CHECK (provenance IS NULL OR json_valid(provenance)),
  created_at TEXT NOT NULL DEFAULT (now()),
  completed_at TEXT,
  CHECK ((((state = 'waiting') AND (front_asset IS NULL) AND (back_asset IS NULL) AND (original_front IS NULL) AND (original_back IS NULL) AND (provenance IS NULL) AND (completed_at IS NULL)) OR ((state = 'ready') AND (front_asset IS NOT NULL) AND (back_asset IS NOT NULL) AND (original_front IS NOT NULL) AND (original_back IS NOT NULL) AND ((length(original_front) >= 1) AND (length(original_front) <= 8388608)) AND ((length(original_back) >= 1) AND (length(original_back) <= 8388608)) AND (provenance IS NOT NULL) AND (completed_at IS NOT NULL)))),
  CHECK ((length(input_sha256)=64 AND input_sha256 NOT GLOB '*[^0-9a-f]*')),
  PRIMARY KEY (id),
  CHECK ((state IN ('waiting', 'ready'))),
  FOREIGN KEY (user_id, magazine_id, back_asset) REFERENCES magazine_artwork(user_id, magazine_id, id) DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE,
  FOREIGN KEY (user_id, magazine_id, front_asset) REFERENCES magazine_artwork(user_id, magazine_id, id) DEFERRABLE INITIALLY DEFERRED,
  UNIQUE (user_id, magazine_id, input_sha256)
 );

CREATE TABLE magazine_covers (
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  body BLOB NOT NULL,
  CHECK (((length(body) >= 1) AND (length(body) <= 5242880))),
  PRIMARY KEY (magazine_id),
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE magazine_exclusions (
  user_id TEXT NOT NULL,
  article_id TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (user_id, article_id),
  FOREIGN KEY (user_id, article_id) REFERENCES articles(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE magazine_origins (
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  provenance JSON NOT NULL CHECK (provenance IS NULL OR json_valid(provenance)),
  PRIMARY KEY (magazine_id),
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE magazine_pdfs (
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  body BLOB NOT NULL,
  sha256 TEXT NOT NULL,
  page_count INTEGER NOT NULL,
  paper_size TEXT NOT NULL,
  renderer_version TEXT NOT NULL,
  verification JSON NOT NULL CHECK (verification IS NULL OR json_valid(verification)),
  created_at TEXT NOT NULL DEFAULT (now()),
  CHECK (((length(body) >= 1) AND (length(body) <= 52428800))),
  CHECK (((page_count >= 1) AND (page_count <= 400))),
  CHECK ((paper_size IN ('Letter', 'A4', 'A5'))),
  PRIMARY KEY (magazine_id),
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE magazine_policy_imports (
  user_id TEXT NOT NULL,
  policy_key TEXT NOT NULL,
  provenance JSON NOT NULL CHECK (provenance IS NULL OR json_valid(provenance)),
  imported_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (user_id, policy_key),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE magazine_production (
  user_id TEXT NOT NULL,
  magazine_id TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT ('queued'),
  stage TEXT NOT NULL DEFAULT ('Waiting for worker'),
  progress INTEGER NOT NULL DEFAULT (0),
  attempts INTEGER NOT NULL DEFAULT (0),
  lease_token TEXT,
  lease_until TEXT,
  error TEXT,
  queued_at TEXT NOT NULL DEFAULT (now()),
  updated_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (magazine_id),
  CHECK (((progress >= 0) AND (progress <= 100))),
  CHECK ((state IN ('queued', 'running', 'waiting_artwork', 'failed', 'done'))),
  FOREIGN KEY (user_id, magazine_id) REFERENCES magazines(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE magazine_publication_exclusions (
  user_id TEXT NOT NULL,
  hostname TEXT NOT NULL,
  provenance JSON NOT NULL DEFAULT ('{}') CHECK (provenance IS NULL OR json_valid(provenance)),
  created_at TEXT NOT NULL DEFAULT (now()),
  CHECK (((hostname = lower(hostname)) AND (regexp('^[a-z0-9][a-z0-9.-]*[a-z0-9]$',hostname)))),
  PRIMARY KEY (user_id, hostname),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE magazines (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  title TEXT NOT NULL,
  subtitle TEXT NOT NULL DEFAULT (''),
  created_at TEXT NOT NULL DEFAULT (now()),
  finalized_at TEXT,
  paper_size TEXT NOT NULL DEFAULT ('Letter'),
  design JSON NOT NULL DEFAULT ('{}') CHECK (design IS NULL OR json_valid(design)),
  CHECK ((paper_size IN ('Letter', 'A4', 'A5'))),
  PRIMARY KEY (id),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  UNIQUE (user_id, id)
 );

CREATE TABLE rate_limits (
  user_id TEXT NOT NULL,
  bucket TEXT NOT NULL,
  window_start TEXT NOT NULL,
  amount INTEGER NOT NULL,
  PRIMARY KEY (user_id, bucket),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE save_receipts (
  id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  request_key TEXT NOT NULL,
  article_id TEXT NOT NULL,
  url TEXT NOT NULL,
  created_article BOOLEAN NOT NULL CHECK (created_article IN (0,1)),
  created_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (id),
  FOREIGN KEY (user_id, article_id) REFERENCES articles(user_id, id) ON DELETE CASCADE,
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  UNIQUE (user_id, request_key)
 );

CREATE TABLE scheduler_accounts (
  user_id TEXT NOT NULL,
  last_claimed_at TEXT NOT NULL DEFAULT ('-infinity'),
  PRIMARY KEY (user_id),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE schema_migrations (
  version TEXT NOT NULL,
  applied_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (version)
 );

CREATE TABLE sources (
  user_id TEXT NOT NULL,
  article_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  source_key TEXT NOT NULL,
  label TEXT NOT NULL,
  received_at TEXT NOT NULL DEFAULT (now()),
  CHECK ((kind IN ('web', 'rss', 'whatsapp'))),
  PRIMARY KEY (user_id, kind, source_key),
  FOREIGN KEY (user_id, article_id) REFERENCES articles(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE whatsapp (
  user_id TEXT NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT (0) CHECK (enabled IN (0,1)),
  phone TEXT,
  status TEXT NOT NULL DEFAULT ('disconnected'),
  qr TEXT,
  qr_expires_at TEXT,
  activated_at TEXT NOT NULL DEFAULT (now()),
  updated_at TEXT NOT NULL DEFAULT (now()),
  error TEXT,
  UNIQUE (phone),
  PRIMARY KEY (user_id),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE whatsapp_keys (
  user_id TEXT NOT NULL,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (user_id, key),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
 );

CREATE TABLE whatsapp_receipts (
  user_id TEXT NOT NULL,
  message_key TEXT NOT NULL,
  article_id TEXT NOT NULL,
  created_article BOOLEAN NOT NULL CHECK (created_article IN (0,1)),
  ack_id TEXT NOT NULL,
  sent_at TEXT,
  created_at TEXT NOT NULL DEFAULT (now()),
  PRIMARY KEY (user_id, message_key),
  FOREIGN KEY (user_id, article_id) REFERENCES articles(user_id, id) ON DELETE CASCADE
 );

CREATE TABLE worker_health (
  id INTEGER NOT NULL,
  heartbeat_at TEXT NOT NULL,
  CHECK ((id = 1)),
  PRIMARY KEY (id)
 );

-- Claims are fenced by a process identity and token, separately from job leases.
CREATE TABLE worker_leader (
  id INTEGER PRIMARY KEY CHECK (id=1),
  token TEXT NOT NULL, process_id INTEGER NOT NULL,
  heartbeat_at TEXT NOT NULL DEFAULT (now()), lease_until TEXT NOT NULL
);

CREATE INDEX article_inbox ON articles (user_id, saved_at DESC);
CREATE INDEX article_publication ON articles(user_id,publication_hostname(url)) WHERE deleted_at IS NULL;
CREATE INDEX article_saved_scope ON articles (user_id, archived_at, read_at, saved_at DESC, id) WHERE (deleted_at IS NULL);
CREATE INDEX feed_capture_content ON feed_captures (user_id, feed_id, content_hash);
CREATE INDEX import_schedule ON import_runs (state, updated_at);
CREATE INDEX pending_jobs ON jobs (state, run_at);
CREATE INDEX magazine_production_queue ON magazine_production (state, queued_at);
CREATE INDEX source_article_kind ON sources (user_id, article_id, kind);

-- A timestamp touch and its revision bump are one trigger, avoiding double bumps.
CREATE TRIGGER article_revision_insert AFTER INSERT ON articles BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER article_revision_update AFTER UPDATE ON articles BEGIN
  UPDATE articles SET updated_at=now() WHERE id=NEW.id;
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER article_revision_delete AFTER DELETE ON articles BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=OLD.user_id;
END;

CREATE TRIGGER feed_revision_insert AFTER INSERT ON feeds BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER feed_revision_update AFTER UPDATE ON feeds BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER feed_revision_delete AFTER DELETE ON feeds BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=OLD.user_id;
END;
CREATE TRIGGER magazine_revision_insert AFTER INSERT ON magazines BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER magazine_revision_update AFTER UPDATE ON magazines BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER magazine_revision_delete AFTER DELETE ON magazines BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=OLD.user_id;
END;
CREATE TRIGGER cover_revision_insert AFTER INSERT ON magazine_covers BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER cover_revision_update AFTER UPDATE ON magazine_covers BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER cover_revision_delete AFTER DELETE ON magazine_covers BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=OLD.user_id;
END;
CREATE TRIGGER magazine_exclusion_revision_insert AFTER INSERT ON magazine_exclusions BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER magazine_exclusion_revision_delete AFTER DELETE ON magazine_exclusions BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=OLD.user_id;
END;
CREATE TRIGGER magazine_publication_exclusion_revision_insert AFTER INSERT ON magazine_publication_exclusions BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=NEW.user_id;
END;
CREATE TRIGGER magazine_publication_exclusion_revision_delete AFTER DELETE ON magazine_publication_exclusions BEGIN
  UPDATE users SET library_revision=library_revision+1 WHERE id=OLD.user_id;
END;

-- Owner erasure may cascade after its parent row disappears. Ordinary finalized edits still fail.
CREATE TRIGGER immutable_issue_articles_insert BEFORE INSERT ON magazine_articles
WHEN EXISTS (SELECT 1 FROM magazines WHERE user_id=NEW.user_id AND id=NEW.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Finalized magazine snapshots are immutable');
END;
CREATE TRIGGER immutable_issue_articles_update BEFORE UPDATE ON magazine_articles
WHEN (EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) OR EXISTS (SELECT 1 FROM magazines WHERE user_id=NEW.user_id AND id=NEW.magazine_id AND finalized_at IS NOT NULL)) BEGIN
  SELECT RAISE(ABORT, 'Finalized magazine snapshots are immutable');
END;
CREATE TRIGGER immutable_issue_articles_delete BEFORE DELETE ON magazine_articles
WHEN EXISTS (SELECT 1 FROM users WHERE id=OLD.user_id) AND EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Finalized magazine snapshots are immutable');
END;
CREATE TRIGGER immutable_issue_metadata BEFORE UPDATE ON magazines
WHEN OLD.finalized_at IS NOT NULL AND (OLD.id IS NOT NEW.id OR OLD.user_id IS NOT NEW.user_id OR OLD.title IS NOT NEW.title OR OLD.subtitle IS NOT NEW.subtitle OR OLD.created_at IS NOT NEW.created_at OR OLD.finalized_at IS NOT NEW.finalized_at OR OLD.paper_size IS NOT NEW.paper_size OR OLD.design IS NOT NEW.design) BEGIN
  SELECT RAISE(ABORT, 'Finalized magazine metadata is immutable');
END;
CREATE TRIGGER immutable_magazine_artwork_insert BEFORE INSERT ON magazine_artwork
WHEN EXISTS (SELECT 1 FROM magazines WHERE user_id=NEW.user_id AND id=NEW.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Magazine artwork is immutable');
END;
CREATE TRIGGER immutable_magazine_artwork_update BEFORE UPDATE ON magazine_artwork
WHEN 1 BEGIN
  SELECT RAISE(ABORT, 'Magazine artwork is immutable');
END;
CREATE TRIGGER immutable_magazine_artwork_delete BEFORE DELETE ON magazine_artwork
WHEN EXISTS (SELECT 1 FROM users WHERE id=OLD.user_id) AND EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Magazine artwork is immutable');
END;
CREATE TRIGGER immutable_magazine_origin_update BEFORE UPDATE ON magazine_origins
WHEN 1 BEGIN
  SELECT RAISE(ABORT, 'Historical magazine provenance is immutable');
END;
CREATE TRIGGER immutable_issue_pdf_update BEFORE UPDATE ON magazine_pdfs
WHEN (EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) OR EXISTS (SELECT 1 FROM magazines WHERE user_id=NEW.user_id AND id=NEW.magazine_id AND finalized_at IS NOT NULL)) BEGIN
  SELECT RAISE(ABORT, 'Finalized magazine PDFs are immutable');
END;
CREATE TRIGGER immutable_magazine_origin_delete BEFORE DELETE ON magazine_origins
WHEN EXISTS (SELECT 1 FROM users WHERE id=OLD.user_id) BEGIN
  SELECT RAISE(ABORT, 'Historical magazine provenance is immutable');
END;
CREATE TRIGGER immutable_issue_pdf_delete BEFORE DELETE ON magazine_pdfs
WHEN EXISTS (SELECT 1 FROM users WHERE id=OLD.user_id) AND EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Finalized magazine PDFs are immutable');
END;
CREATE TRIGGER immutable_magazine_cover_request_insert BEFORE INSERT ON magazine_cover_requests
WHEN EXISTS (SELECT 1 FROM magazines WHERE user_id=NEW.user_id AND id=NEW.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Generated cover request is immutable');
END;
CREATE TRIGGER immutable_magazine_cover_request_update BEFORE UPDATE ON magazine_cover_requests
WHEN (EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) OR EXISTS (SELECT 1 FROM magazines WHERE user_id=NEW.user_id AND id=NEW.magazine_id AND finalized_at IS NOT NULL) OR OLD.state='ready' OR OLD.id IS NOT NEW.id OR OLD.user_id IS NOT NEW.user_id OR OLD.magazine_id IS NOT NEW.magazine_id OR OLD.input_sha256 IS NOT NEW.input_sha256 OR OLD.request IS NOT NEW.request OR OLD.created_at IS NOT NEW.created_at) BEGIN
  SELECT RAISE(ABORT, 'Generated cover request is immutable');
END;
CREATE TRIGGER immutable_magazine_cover_request_delete BEFORE DELETE ON magazine_cover_requests
WHEN EXISTS (SELECT 1 FROM users WHERE id=OLD.user_id) AND EXISTS (SELECT 1 FROM magazines WHERE user_id=OLD.user_id AND id=OLD.magazine_id AND finalized_at IS NOT NULL) BEGIN
  SELECT RAISE(ABORT, 'Generated cover request is immutable');
END;
