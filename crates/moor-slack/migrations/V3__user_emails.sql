CREATE TABLE slack_user_emails (
    slack_user TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);
