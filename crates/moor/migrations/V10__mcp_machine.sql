ALTER TABLE user_mcp_servers ADD COLUMN machine INTEGER NOT NULL DEFAULT 0 CHECK (machine IN (0, 1));
