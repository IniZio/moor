-- +goose Up
-- A trigger is set when all three trigger columns are, and the key then holds once per project.
-- Context and event data are json, not jsonb, because the core never reads them and must get the bytes back.
CREATE TABLE tasks (
    id text PRIMARY KEY,
    project text NOT NULL,
    prompt text NOT NULL,
    kind text NOT NULL DEFAULT '',
    started_by text NOT NULL DEFAULT '',
    trigger_source text,
    trigger_group text,
    trigger_rev text,
    trigger_context json,
    state text NOT NULL CHECK (state IN
        ('waiting', 'starting', 'running', 'delivering', 'done', 'failed', 'superseded', 'cancelled')),
    version bigint NOT NULL CHECK (version > 0),
    pins jsonb,
    refs jsonb,
    runner_id text NOT NULL DEFAULT '',
    run bigint NOT NULL DEFAULT 0,
    gen bigint NOT NULL DEFAULT 0,
    last_seq bigint NOT NULL DEFAULT 0,
    deadline timestamptz,
    keep_until timestamptz,
    failure text NOT NULL DEFAULT '',
    token_owner text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL,
    ended_at timestamptz,
    CONSTRAINT tasks_trigger_whole CHECK (
        (trigger_source IS NULL) = (trigger_group IS NULL) AND (trigger_source IS NULL) = (trigger_rev IS NULL))
);

CREATE UNIQUE INDEX tasks_trigger_key ON tasks (project, trigger_source, trigger_group, trigger_rev)
    WHERE trigger_source IS NOT NULL;

CREATE TABLE task_events (
    task_id text NOT NULL REFERENCES tasks (id),
    seq bigint NOT NULL CHECK (seq > 0),
    at timestamptz NOT NULL,
    kind text NOT NULL,
    data json,
    PRIMARY KEY (task_id, seq)
);

CREATE TABLE result_versions (
    task_id text NOT NULL REFERENCES tasks (id),
    version integer NOT NULL CHECK (version > 0),
    entry_url text NOT NULL,
    line text NOT NULL,
    digest text NOT NULL,
    published_at timestamptz NOT NULL,
    PRIMARY KEY (task_id, version),
    UNIQUE (task_id, digest)
);
