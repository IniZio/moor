CREATE TABLE setup_checkpoints_v44 (provider TEXT NOT NULL, layer_key TEXT NOT NULL,
  checkpoint TEXT NOT NULL, repo TEXT NOT NULL, bytes INTEGER, created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL, workdir TEXT, setup_ms INTEGER, user INTEGER, chain TEXT,
  chain_at INTEGER, parent TEXT, evicting INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (provider, checkpoint));
INSERT INTO setup_checkpoints_v44 (provider, layer_key, checkpoint, repo, bytes, created_at,
    last_used_at, workdir, setup_ms, user, chain, chain_at)
  SELECT provider, layer_key, checkpoint, repo, bytes, created_at, last_used_at, workdir,
    setup_ms, user, chain, chain_at FROM setup_checkpoints;
DROP TABLE setup_checkpoints;
ALTER TABLE setup_checkpoints_v44 RENAME TO setup_checkpoints;
CREATE UNIQUE INDEX setup_checkpoints_layer ON setup_checkpoints (provider, layer_key)
  WHERE evicting = 0;
CREATE INDEX setup_checkpoints_parent ON setup_checkpoints (provider, parent);
