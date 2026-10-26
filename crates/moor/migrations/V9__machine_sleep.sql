ALTER TABLE box_machines ADD COLUMN asleep INTEGER NOT NULL DEFAULT 0 CHECK (asleep IN (0, 1));
DROP INDEX box_machines_slot;
CREATE UNIQUE INDEX box_machines_slot ON box_machines (user, machine, slot) WHERE asleep = 0;
