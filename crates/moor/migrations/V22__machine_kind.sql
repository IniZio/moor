ALTER TABLE user_machines ADD COLUMN kind TEXT NOT NULL DEFAULT 'physical'
  CHECK (kind IN ('physical', 'vm-host'));
ALTER TABLE user_machines ADD COLUMN vm_provider TEXT
  CHECK ((kind = 'vm-host') = (vm_provider IS NOT NULL));
