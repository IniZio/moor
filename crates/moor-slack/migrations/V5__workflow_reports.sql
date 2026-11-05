CREATE TABLE workflow_cursor (
    owner TEXT PRIMARY KEY,
    after INTEGER NOT NULL
);
CREATE TABLE workflow_skips (
    owner TEXT NOT NULL,
    workflow TEXT NOT NULL,
    repo TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (owner, workflow, repo, kind)
);
CREATE TABLE workflow_outbox (
    id INTEGER PRIMARY KEY,
    owner TEXT NOT NULL,
    channel TEXT NOT NULL,
    thread_ts TEXT NOT NULL,
    text TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0
);
