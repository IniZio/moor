-- +goose Up
ALTER TABLE tasks ADD COLUMN default_branch text;
