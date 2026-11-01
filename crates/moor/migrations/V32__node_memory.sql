ALTER TABLE machine_links ADD COLUMN mem_mib INTEGER;
ALTER TABLE machine_links ADD COLUMN cpus INTEGER;
ALTER TABLE machine_links ADD COLUMN reserve_mib INTEGER;
ALTER TABLE boxes ADD COLUMN node_mem_mib INTEGER;
