-- +goose Up
CREATE TABLE task_home_commits (
    task_id text NOT NULL REFERENCES tasks (id),
    n integer NOT NULL CHECK (n > 0),
    commit text NOT NULL,
    at timestamptz NOT NULL,
    cause text NOT NULL CHECK (cause IN ('start', 'push')),
    PRIMARY KEY (task_id, n)
);
