ALTER TABLE user_machines DROP COLUMN vm_provider;
ALTER TABLE user_machines DROP COLUMN kind;
ALTER TABLE user_machines ADD COLUMN pool_provider TEXT;
ALTER TABLE user_machines_v2 ADD COLUMN pool_provider TEXT;
