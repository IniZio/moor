ALTER TABLE user_machines ADD COLUMN capacity INTEGER NOT NULL DEFAULT 1 CHECK (capacity >= 1);
ALTER TABLE box_machines ADD COLUMN slot INTEGER NOT NULL DEFAULT 0;
DELETE FROM box_machines WHERE box IN (SELECT id FROM boxes WHERE state IN ('closed','rejected'));
CREATE UNIQUE INDEX box_machines_slot ON box_machines (user, machine, slot);
