ALTER TABLE machine_links ADD COLUMN max_boxes INTEGER;
ALTER TABLE machine_links ADD COLUMN cache_gb INTEGER;
ALTER TABLE machine_links ADD COLUMN pool_features TEXT NOT NULL DEFAULT '[]';
