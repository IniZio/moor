-- +goose Up
-- A spec and an event are each set whole, an event needs a spec, and the key holds per project and spec.
-- Context and event data are json, not jsonb, because the core never reads them and must get the bytes back.
CREATE TABLE tasks (
    id text PRIMARY KEY,
    project text NOT NULL,
    prompt text NOT NULL,
    review_prompt text NOT NULL DEFAULT '',
    started_by text NOT NULL DEFAULT '',
    spec_name text,
    spec_file text,
    event_source text,
    event_group text,
    event_rev text,
    event_context json,
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
    CONSTRAINT tasks_spec_whole CHECK ((spec_name IS NULL) = (spec_file IS NULL)),
    CONSTRAINT tasks_event_whole CHECK (
        (event_source IS NULL) = (event_group IS NULL) AND (event_source IS NULL) = (event_rev IS NULL)),
    CONSTRAINT tasks_event_has_spec CHECK (event_source IS NULL OR spec_name IS NOT NULL)
);

CREATE UNIQUE INDEX tasks_event_key ON tasks (project, spec_name, event_source, event_group, event_rev)
    WHERE event_source IS NOT NULL;

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
