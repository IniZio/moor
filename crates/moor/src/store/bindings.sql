CREATE TABLE IF NOT EXISTS bindings (front TEXT NOT NULL, conv_key TEXT NOT NULL,
  session INTEGER NOT NULL REFERENCES sessions(id), owner TEXT NOT NULL, status_ts TEXT,
  created_at INTEGER NOT NULL, PRIMARY KEY (front, conv_key));
CREATE INDEX IF NOT EXISTS bindings_session ON bindings (session);
