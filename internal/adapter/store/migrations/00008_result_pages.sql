-- +goose Up
CREATE TABLE result_pages (
    task_id text NOT NULL,
    version integer NOT NULL,
    page bytea NOT NULL,
    PRIMARY KEY (task_id, version),
    FOREIGN KEY (task_id, version) REFERENCES result_versions (task_id, version)
);
