ALTER TABLE user_webhooks ADD COLUMN state TEXT NOT NULL DEFAULT 'waiting_for_secret';
ALTER TABLE user_webhooks ADD COLUMN header TEXT;
ALTER TABLE user_webhooks ADD COLUMN header_value BLOB;
