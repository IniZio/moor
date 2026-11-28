-- +goose Up
ALTER TABLE tasks RENAME COLUMN runner_id TO machine_id;

ALTER INDEX tasks_on_runner RENAME TO tasks_on_machine;
