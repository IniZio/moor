-- +goose Up
CREATE TABLE machines (
    id text PRIMARY KEY,
    name text UNIQUE,
    enrolled_by text NOT NULL,
    pubkey bytea,
    join_hash bytea UNIQUE,
    join_expires_at timestamptz,
    build text NOT NULL DEFAULT '',
    link_gen bigint NOT NULL DEFAULT 0,
    holder_addr text NOT NULL DEFAULT '',
    heard_at timestamptz,
    draining_since timestamptz,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL,
    CONSTRAINT machines_joined_whole CHECK ((pubkey IS NULL) = (name IS NULL)),
    CONSTRAINT machines_token_whole CHECK ((join_hash IS NULL) = (join_expires_at IS NULL))
);

CREATE INDEX tasks_on_runner ON tasks (runner_id, created_at) WHERE state IN ('starting', 'running', 'delivering');

CREATE INDEX tasks_waiting ON tasks (created_at, id) WHERE state = 'waiting';
