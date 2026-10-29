CREATE TABLE user_machines_v2 (machine_id INTEGER PRIMARY KEY,
  user INTEGER NOT NULL REFERENCES users(id), name TEXT NOT NULL, pubkey TEXT NOT NULL,
  labels TEXT NOT NULL DEFAULT '[]', capacity INTEGER NOT NULL CHECK (capacity >= 1),
  enrolled_at INTEGER NOT NULL, UNIQUE (user, name));
