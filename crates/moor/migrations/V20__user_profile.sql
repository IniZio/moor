CREATE TABLE user_profiles (user INTEGER NOT NULL PRIMARY KEY REFERENCES users(id),
  repo TEXT NOT NULL, git_ref TEXT, updated_at INTEGER NOT NULL);
