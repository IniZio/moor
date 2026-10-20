CREATE TABLE IF NOT EXISTS slack_events (event_id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS bindings (front TEXT NOT NULL, conv_key TEXT NOT NULL,
  session INTEGER NOT NULL REFERENCES sessions(id), owner TEXT NOT NULL, status_ts TEXT,
  created_at INTEGER NOT NULL, PRIMARY KEY (front, conv_key));
CREATE INDEX IF NOT EXISTS bindings_session ON bindings (session);
CREATE TABLE IF NOT EXISTS slack_prompts (event_id INTEGER PRIMARY KEY, channel TEXT NOT NULL, thread_ts TEXT NOT NULL,
  message_ts TEXT NOT NULL, session INTEGER NOT NULL REFERENCES sessions(id), question TEXT NOT NULL,
  options TEXT NOT NULL, answer TEXT, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS slack_prompts_message ON slack_prompts (channel, message_ts);
CREATE TABLE IF NOT EXISTS slack_artifacts (box INTEGER NOT NULL REFERENCES boxes(id), name TEXT NOT NULL,
  version INTEGER NOT NULL, posted_at INTEGER NOT NULL, PRIMARY KEY (box, name, version));
