ALTER TABLE machine_links ADD COLUMN psi_full_centi INTEGER;
ALTER TABLE boxes ADD COLUMN resume_first_at INTEGER;
CREATE TABLE node_pressure (
    node TEXT PRIMARY KEY,
    readings TEXT NOT NULL,
    high_since INTEGER,
    suspended_at INTEGER
);
