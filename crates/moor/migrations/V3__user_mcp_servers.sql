CREATE TABLE user_mcp_servers (user INTEGER NOT NULL REFERENCES users(id), name TEXT NOT NULL,
  url TEXT NOT NULL, header TEXT NOT NULL, secret TEXT NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (user, name));
