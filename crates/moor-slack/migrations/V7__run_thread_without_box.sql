CREATE TABLE workflow_run_threads_v7 (
    box_name TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    workflow TEXT NOT NULL,
    repo TEXT NOT NULL,
    channel TEXT NOT NULL,
    origin_ts TEXT NOT NULL,
    box_id INTEGER,
    branch TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    opened INTEGER NOT NULL DEFAULT 0,
    thread_ts TEXT
);
INSERT INTO workflow_run_threads_v7
    (box_name, owner, workflow, repo, channel, origin_ts, box_id, branch, attempts, opened, thread_ts)
SELECT t.box_name, t.owner, t.workflow, t.repo, t.channel, t.origin_ts, t.box_id, t.branch,
       t.attempts, t.opened,
       (SELECT b.thread_ts FROM slack_boxes b WHERE b.box_name = t.box_name AND t.opened = 1)
FROM workflow_run_threads t;
DROP TABLE workflow_run_threads;
ALTER TABLE workflow_run_threads_v7 RENAME TO workflow_run_threads;
