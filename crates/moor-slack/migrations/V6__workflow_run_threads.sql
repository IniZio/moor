CREATE TABLE workflow_run_threads (
    box_name TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    workflow TEXT NOT NULL,
    repo TEXT NOT NULL,
    channel TEXT NOT NULL,
    origin_ts TEXT NOT NULL,
    box_id INTEGER NOT NULL,
    branch TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    opened INTEGER NOT NULL DEFAULT 0
);
ALTER TABLE workflow_outbox ADD COLUMN box_name TEXT;
