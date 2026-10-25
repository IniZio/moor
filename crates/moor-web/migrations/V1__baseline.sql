CREATE TABLE requests (
    key TEXT PRIMARY KEY,
    root_session TEXT NOT NULL,
    repo TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    ended_at INTEGER
);
CREATE TABLE cursor (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    value INTEGER NOT NULL
);
