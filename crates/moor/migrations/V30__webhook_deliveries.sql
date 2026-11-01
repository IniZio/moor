CREATE TABLE webhook_deliveries (id INTEGER PRIMARY KEY, webhook INTEGER NOT NULL REFERENCES user_webhooks(id), event INTEGER NOT NULL REFERENCES events(id), leased_until INTEGER);
CREATE INDEX webhook_deliveries_webhook ON webhook_deliveries(webhook, id);
