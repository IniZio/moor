CREATE TABLE user_mcp_oauth (
    user INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    value BLOB NOT NULL,
    expires_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user, name)
);
CREATE TABLE mcp_oauth_pending (
    state TEXT PRIMARY KEY,
    user INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    value BLOB NOT NULL,
    expires_at INTEGER NOT NULL
);
