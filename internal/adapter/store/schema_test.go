package store

import (
	"context"
	"testing"
)

// The data page names the column of the machine that runs a task machine_id.
func TestATaskNamesItsMachineInMachineID(t *testing.T) {
	s := freshStore(t)
	n := count(t, s, "SELECT count(*) FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'tasks' AND column_name = 'machine_id'")
	if n != 1 {
		t.Fatalf("tasks has %d columns named machine_id, want 1", n)
	}
	if n := count(t, s, "SELECT count(*) FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'tasks' AND column_name = 'runner_id'"); n != 0 {
		t.Fatalf("tasks still has a runner_id column")
	}
	if _, err := s.pool.Exec(context.Background(), "SELECT machine_id FROM tasks LIMIT 1"); err != nil {
		t.Fatal(err)
	}
}
