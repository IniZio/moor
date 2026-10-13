CREATE TABLE meta   (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE tasks  (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, repo TEXT NOT NULL, issue TEXT, goal TEXT, base TEXT,
  branch TEXT, worktree TEXT, bound INTEGER NOT NULL DEFAULT 0,
  workspace_id TEXT, agent_session_id TEXT,
  state TEXT NOT NULL, state_reason TEXT,
  intent_kind TEXT, intent_pid INTEGER, intent_run INTEGER, intent_at INTEGER, setup_failed_at INTEGER,
  idle_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE UNIQUE INDEX tasks_open ON tasks(name) WHERE state NOT IN ('closed','rejected');
CREATE TABLE boxes  (
  task INTEGER PRIMARY KEY REFERENCES tasks(id), provider TEXT NOT NULL, handle TEXT, state TEXT NOT NULL,
  attempted_handle TEXT, workdir TEXT, egress_enforced INTEGER,
  sleep_reason TEXT, updated_at INTEGER NOT NULL);
CREATE TABLE events (id INTEGER PRIMARY KEY, task INTEGER NOT NULL REFERENCES tasks(id), kind TEXT NOT NULL,
  payload TEXT NOT NULL, ts INTEGER NOT NULL);
CREATE INDEX events_task ON events(task, id);
CREATE TABLE sessions (id INTEGER PRIMARY KEY, task INTEGER NOT NULL, session_ref TEXT NOT NULL, started_at INTEGER NOT NULL,
  ended_at INTEGER, exit INTEGER, transcript_digest TEXT);
CREATE TABLE runs   (id INTEGER PRIMARY KEY, task INTEGER NOT NULL, argv TEXT NOT NULL, record TEXT, sha TEXT, sync TEXT NOT NULL,
  pid INTEGER NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, exit INTEGER, interrupted INTEGER NOT NULL DEFAULT 0);
CREATE TABLE reports(id INTEGER PRIMARY KEY, task INTEGER NOT NULL, kind TEXT NOT NULL, version INTEGER NOT NULL, path TEXT NOT NULL,
  title TEXT, created_at INTEGER NOT NULL, UNIQUE (task, kind, version));
CREATE TABLE hook_cache (task INTEGER PRIMARY KEY, record TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE proxy  (task INTEGER PRIMARY KEY, pid INTEGER, host_port INTEGER, box_port INTEGER, state TEXT NOT NULL, updated_at INTEGER NOT NULL);
INSERT INTO meta (k, v) VALUES ('schema_version', '1');
