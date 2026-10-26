CREATE TABLE bindings_history (front TEXT NOT NULL, conv_key TEXT NOT NULL,
  session INTEGER NOT NULL REFERENCES sessions(id), owner TEXT NOT NULL,
  created_at INTEGER NOT NULL, PRIMARY KEY (front, conv_key, session));
INSERT INTO bindings_history (front, conv_key, session, owner, created_at)
  SELECT front, conv_key, session, owner, created_at FROM bindings ORDER BY created_at, rowid;
DROP TABLE bindings;
ALTER TABLE bindings_history RENAME TO bindings;
CREATE INDEX bindings_session ON bindings (session);
