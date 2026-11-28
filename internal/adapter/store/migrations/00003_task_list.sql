-- +goose Up
CREATE SEQUENCE task_numbers;

CREATE INDEX tasks_created_at ON tasks (created_at DESC);
