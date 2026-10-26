CREATE TABLE user_machines (user INTEGER NOT NULL REFERENCES users(id), name TEXT NOT NULL,
  ssh TEXT NOT NULL, labels TEXT NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (user, name));
