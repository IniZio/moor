-- +goose Up
ALTER TABLE blocked_requests
    ADD COLUMN path text NOT NULL DEFAULT '',
    ADD COLUMN rule text NOT NULL DEFAULT '',
    ADD COLUMN entry text NOT NULL DEFAULT '',
    ADD COLUMN detail text NOT NULL DEFAULT '',
    DROP CONSTRAINT blocked_requests_task_id_proxy_host_port_method_key,
    ADD CONSTRAINT blocked_requests_one_row UNIQUE (task_id, proxy, host, port, method, path, detail);
