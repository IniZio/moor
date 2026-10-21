CREATE TABLE meta   (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE boxes  (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, repo TEXT NOT NULL, user INTEGER REFERENCES users(id),
  issue TEXT, goal TEXT, base TEXT,
  branch TEXT, worktree TEXT,
  state TEXT NOT NULL, state_reason TEXT,
  intent_kind TEXT, intent_pid INTEGER, intent_run INTEGER, intent_at INTEGER, setup_failed_at INTEGER,
  idle_since INTEGER, launch_failure TEXT,
  provider TEXT, handle TEXT, driver_state TEXT, attempted_handle TEXT, workdir TEXT, egress TEXT, sleep_reason TEXT,
  driver_updated_at INTEGER,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE UNIQUE INDEX boxes_open ON boxes(name) WHERE state NOT IN ('closed','rejected');
CREATE TABLE sessions (id INTEGER PRIMARY KEY, box INTEGER REFERENCES boxes(id), ordinal INTEGER NOT NULL,
  agent TEXT, model TEXT, agent_session_id TEXT, workspace_id TEXT, pane TEXT, state TEXT NOT NULL,
  started_at INTEGER NOT NULL, ended_at INTEGER, parent_session INTEGER REFERENCES sessions(id), workdir TEXT,
  user INTEGER REFERENCES users(id),
  CHECK (box IS NOT NULL OR workdir IS NOT NULL), UNIQUE (box, ordinal));
CREATE INDEX sessions_parent ON sessions(parent_session);
CREATE TABLE launches (id INTEGER PRIMARY KEY, box INTEGER NOT NULL REFERENCES boxes(id), session_ref TEXT NOT NULL, pane TEXT,
  started_at INTEGER NOT NULL, ended_at INTEGER, exit INTEGER, transcript_digest TEXT, registered_agent_session TEXT,
  session INTEGER REFERENCES sessions(id));
CREATE TABLE box_refs (box INTEGER NOT NULL REFERENCES boxes(id), kind TEXT NOT NULL CHECK (kind IN ('branch','pr','proof')),
  value TEXT NOT NULL, PRIMARY KEY (box, kind, value));
CREATE TABLE events (id INTEGER PRIMARY KEY, box INTEGER NOT NULL REFERENCES boxes(id), session INTEGER REFERENCES sessions(id),
  kind TEXT NOT NULL, payload TEXT NOT NULL, ts INTEGER NOT NULL);
CREATE INDEX events_box ON events(box, id);
CREATE TABLE runs   (id INTEGER PRIMARY KEY, box INTEGER NOT NULL, argv TEXT NOT NULL, record TEXT, sha TEXT,
  pid INTEGER NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, exit INTEGER, interrupted INTEGER NOT NULL DEFAULT 0);
CREATE TABLE reports(id INTEGER PRIMARY KEY, box INTEGER NOT NULL, session INTEGER REFERENCES sessions(id), kind TEXT NOT NULL,
  version INTEGER NOT NULL, path TEXT NOT NULL, title TEXT, created_at INTEGER NOT NULL, UNIQUE (box, kind, version));
CREATE TABLE hook_cache (session INTEGER PRIMARY KEY REFERENCES sessions(id), record TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE proxy  (box INTEGER PRIMARY KEY REFERENCES boxes(id), box_port INTEGER NOT NULL UNIQUE,
  pid INTEGER, handle TEXT, ready INTEGER NOT NULL DEFAULT 0, lease_owner TEXT, lease_heartbeat INTEGER,
  boot_id TEXT, updated_at INTEGER NOT NULL);
CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, created_at INTEGER NOT NULL);
CREATE TABLE identities (identity TEXT PRIMARY KEY, user INTEGER NOT NULL REFERENCES users(id),
  linked_at INTEGER NOT NULL);
CREATE INDEX identities_user ON identities(user);
CREATE TABLE link_codes (code TEXT PRIMARY KEY, user INTEGER NOT NULL REFERENCES users(id),
  expires_at INTEGER NOT NULL);
CREATE TABLE user_secrets (user INTEGER NOT NULL REFERENCES users(id), name TEXT NOT NULL, value BLOB NOT NULL,
  updated_at INTEGER NOT NULL, PRIMARY KEY (user, name));
INSERT INTO meta (k, v) VALUES ('schema_version', '9');
