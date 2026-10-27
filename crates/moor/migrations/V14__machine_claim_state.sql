CREATE TABLE box_machines_v11 (box INTEGER PRIMARY KEY REFERENCES boxes(id),
  user INTEGER NOT NULL REFERENCES users(id),
  state TEXT NOT NULL CHECK (state IN ('wanted', 'held', 'asleep')),
  machine TEXT, labels TEXT NOT NULL DEFAULT '[]', ports TEXT NOT NULL,
  slot INTEGER NOT NULL DEFAULT 0, sync_exclude TEXT NOT NULL DEFAULT '[]',
  since INTEGER NOT NULL DEFAULT 0, until INTEGER,
  CHECK (state = 'wanted' OR machine IS NOT NULL));
INSERT INTO box_machines_v11 (box, user, state, machine, ports, slot, sync_exclude)
  SELECT box, user, CASE asleep WHEN 1 THEN 'asleep' ELSE 'held' END, machine, ports, slot,
    sync_exclude FROM box_machines;
DROP TABLE box_machines;
ALTER TABLE box_machines_v11 RENAME TO box_machines;
CREATE UNIQUE INDEX box_machines_slot ON box_machines (user, machine, slot) WHERE state = 'held';
CREATE INDEX box_machines_wanted ON box_machines (user, since) WHERE state = 'wanted';
