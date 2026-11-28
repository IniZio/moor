-- +goose Up
CREATE TABLE settings_documents (
    version integer PRIMARY KEY CHECK (version > 0),
    text text NOT NULL,
    note text NOT NULL DEFAULT '',
    saved_by text NOT NULL,
    saved_at timestamptz NOT NULL
);

CREATE TABLE secret_values (
    connection text PRIMARY KEY,
    ciphertext bytea NOT NULL,
    key_id text NOT NULL,
    updated_at timestamptz NOT NULL
);
