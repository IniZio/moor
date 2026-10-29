CREATE TABLE machine_links (machine_id INTEGER PRIMARY KEY REFERENCES user_machines_v2(machine_id) ON DELETE CASCADE, online INTEGER NOT NULL, since INTEGER NOT NULL);
