CREATE TABLE IF NOT EXISTS slack_events (event_id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS slack_threads (channel TEXT NOT NULL, thread_ts TEXT NOT NULL, task TEXT NOT NULL,
  created_at INTEGER NOT NULL, PRIMARY KEY (channel, thread_ts));
