CREATE TABLE IF NOT EXISTS bindings (front TEXT NOT NULL, conv_key TEXT NOT NULL,
  session INTEGER NOT NULL REFERENCES sessions(id), owner TEXT NOT NULL,
  created_at INTEGER NOT NULL, PRIMARY KEY (front, conv_key));
CREATE INDEX IF NOT EXISTS bindings_session ON bindings (session);
CREATE TABLE IF NOT EXISTS box_lease (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  box INTEGER NOT NULL REFERENCES boxes(id),
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  state TEXT NOT NULL,
  booted INTEGER NOT NULL DEFAULT 0,
  pgid INTEGER, sid INTEGER, root_pid INTEGER, root_start INTEGER,
  size_at_release INTEGER,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE(box, kind, key));
CREATE INDEX IF NOT EXISTS box_lease_box ON box_lease(box);
CREATE TABLE IF NOT EXISTS artifacts (id INTEGER PRIMARY KEY, box INTEGER NOT NULL REFERENCES boxes(id), session INTEGER REFERENCES sessions(id), name TEXT NOT NULL, version INTEGER NOT NULL, kind TEXT NOT NULL, content_type TEXT NOT NULL, size INTEGER NOT NULL, sha256 TEXT NOT NULL, path TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE (box, name, version));
CREATE TABLE IF NOT EXISTS artifact_cursors (box INTEGER PRIMARY KEY REFERENCES boxes(id), through INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS front_leases (token_hash TEXT PRIMARY KEY, front TEXT NOT NULL,
  user INTEGER NOT NULL REFERENCES users(id), expires_at INTEGER NOT NULL);
