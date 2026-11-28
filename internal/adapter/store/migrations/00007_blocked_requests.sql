-- +goose Up
-- One row per task, proxy, host, port and method a proxy refused. A repeat counts on its row.
CREATE TABLE blocked_requests (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    task_id text NOT NULL,
    proxy text NOT NULL CHECK (proxy IN ('box', 'desk')),
    host text NOT NULL,
    port integer NOT NULL CHECK (port BETWEEN 1 AND 65535),
    method text NOT NULL,
    reason text NOT NULL,
    repo text NOT NULL DEFAULT '',
    count integer NOT NULL DEFAULT 1 CHECK (count > 0),
    first_at timestamptz NOT NULL,
    last_at timestamptz NOT NULL,
    decision text NOT NULL DEFAULT '' CHECK (decision IN ('', 'allowed', 'dismissed')),
    decided_by text NOT NULL DEFAULT '',
    decided_at timestamptz,
    UNIQUE (task_id, proxy, host, port, method)
);
