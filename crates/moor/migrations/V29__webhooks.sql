CREATE TABLE user_webhooks (id INTEGER PRIMARY KEY, user INTEGER NOT NULL REFERENCES users(id), url TEXT NOT NULL, events TEXT NOT NULL, key BLOB NOT NULL, created_at INTEGER NOT NULL, last_ok_at INTEGER, failing_since INTEGER, last_error TEXT);
CREATE INDEX user_webhooks_user ON user_webhooks(user);
