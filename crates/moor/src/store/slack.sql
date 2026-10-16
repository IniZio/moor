CREATE TABLE IF NOT EXISTS slack_events (event_id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS slack_threads (channel TEXT NOT NULL, thread_ts TEXT NOT NULL, task TEXT NOT NULL,
  owner TEXT NOT NULL, status_ts TEXT, created_at INTEGER NOT NULL, PRIMARY KEY (channel, thread_ts));
CREATE INDEX IF NOT EXISTS slack_threads_task ON slack_threads (task);
CREATE TABLE IF NOT EXISTS slack_prompts (event_id INTEGER PRIMARY KEY, channel TEXT NOT NULL, thread_ts TEXT NOT NULL,
  message_ts TEXT NOT NULL, task TEXT NOT NULL, question TEXT NOT NULL, options TEXT NOT NULL, answer TEXT,
  created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS slack_prompts_message ON slack_prompts (channel, message_ts);
