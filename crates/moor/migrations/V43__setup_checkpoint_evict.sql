ALTER TABLE setup_checkpoints ADD COLUMN user INTEGER;
ALTER TABLE setup_checkpoints ADD COLUMN chain TEXT;
ALTER TABLE setup_checkpoints ADD COLUMN chain_at INTEGER;
CREATE TABLE setup_checkpoint_pins (box_id INTEGER NOT NULL, provider TEXT NOT NULL, layer_key TEXT NOT NULL, layer INTEGER NOT NULL, PRIMARY KEY (box_id, provider, layer_key));
