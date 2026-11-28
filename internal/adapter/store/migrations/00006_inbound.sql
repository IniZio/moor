-- +goose Up
CREATE TABLE inbound_events (
    source text NOT NULL,
    delivery_id text NOT NULL,
    event text NOT NULL,
    action text NOT NULL DEFAULT '',
    received_at timestamptz NOT NULL,
    outcome text NOT NULL DEFAULT '',
    PRIMARY KEY (source, delivery_id)
);

CREATE TABLE inbound_event_tasks (
    source text NOT NULL,
    delivery_id text NOT NULL,
    task_id text NOT NULL REFERENCES tasks (id),
    PRIMARY KEY (source, delivery_id, task_id),
    FOREIGN KEY (source, delivery_id) REFERENCES inbound_events (source, delivery_id)
);

CREATE INDEX inbound_event_tasks_task ON inbound_event_tasks (task_id);

CREATE TABLE deliveries (
    target_kind text NOT NULL,
    target_ref text NOT NULL,
    external_id text NOT NULL DEFAULT '',
    sections jsonb NOT NULL DEFAULT '{}',
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (target_kind, target_ref)
);
