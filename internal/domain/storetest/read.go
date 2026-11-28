package storetest

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// ReadStore is a task store that also serves the portal's streams: a task's event rows after a seq, and a stamp
// that changes whenever any task is written.
type ReadStore interface {
	Store
	EventsAfter(ctx context.Context, id domain.TaskID, after int64, limit int) ([]domain.EventRow, error)
	TaskStamp(ctx context.Context) (string, error)
}

// RunRead runs the stream-reading cases on a fresh store each.
func RunRead(t *testing.T, fresh func(t *testing.T) ReadStore) {
	t.Helper()
	cases := []struct {
		name string
		run  func(*testing.T, ReadStore)
	}{
		{"events after a seq come in order", eventsAfter},
		{"events are cut by the limit and continue from the last seq", eventsLimit},
		{"a task with no later events or no row gives none", eventsNone},
		{"the stamp changes with every write and with a new task", stampChanges},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) { c.run(t, fresh(t)) })
	}
}

func after(t *testing.T, s ReadStore, id domain.TaskID, seq int64, limit int) []domain.EventRow {
	t.Helper()
	rows, err := s.EventsAfter(context.Background(), id, seq, limit)
	if err != nil {
		t.Fatalf("events of %s after %d: %v", id, seq, err)
	}
	return rows
}

func eventsAfter(t *testing.T, s ReadStore) {
	create(t, Subject{Store: s}, func() domain.Task { x := person("1"); x.LastSeq = 5; return x }())
	create(t, Subject{Store: s}, func() domain.Task { x := person("2"); x.LastSeq = 3; return x }())
	sameEvents(t, "all of task 1", after(t, s, "1", 0, 100), eventsFrom("1", 1, 5))
	sameEvents(t, "after 2", after(t, s, "1", 2, 100), eventsFrom("1", 3, 5))
	sameEvents(t, "after the last", after(t, s, "1", 5, 100), nil)
	sameEvents(t, "task 2 is apart", after(t, s, "2", 0, 100), eventsFrom("2", 1, 3))
}

func eventsLimit(t *testing.T, s ReadStore) {
	create(t, Subject{Store: s}, func() domain.Task { x := person("1"); x.LastSeq = 7; return x }())
	first := after(t, s, "1", 0, 3)
	sameEvents(t, "first page", first, eventsFrom("1", 1, 3))
	second := after(t, s, "1", first[len(first)-1].Seq, 3)
	sameEvents(t, "second page", second, eventsFrom("1", 4, 6))
	sameEvents(t, "last page", after(t, s, "1", second[len(second)-1].Seq, 3), eventsFrom("1", 7, 7))
}

func eventsNone(t *testing.T, s ReadStore) {
	sameEvents(t, "no such task", after(t, s, "9", 0, 10), nil)
	create(t, Subject{Store: s}, person("1"))
	sameEvents(t, "after the only row", after(t, s, "1", 1, 10), nil)
}

func stampChanges(t *testing.T, s ReadStore) {
	ctx := context.Background()
	stamp := func() string {
		t.Helper()
		got, err := s.TaskStamp(ctx)
		if err != nil {
			t.Fatalf("stamp: %v", err)
		}
		return got
	}
	empty := stamp()
	if again := stamp(); again != empty {
		t.Errorf("an unchanged table has stamps %q and %q", empty, again)
	}
	saved := create(t, Subject{Store: s}, person("1"))
	created := stamp()
	if created == empty {
		t.Errorf("creating a task left the stamp %q", created)
	}
	saved.State, saved.LastSeq = domain.Starting, 2
	if _, err := s.Save(ctx, domain.TaskWrite{Task: saved, Events: eventsFrom("1", 2, 2)}); err != nil {
		t.Fatal(err)
	}
	written := stamp()
	if written == created || written == empty {
		t.Errorf("writing a task gave the stamp %q after %q and %q", written, created, empty)
	}
	create(t, Subject{Store: s}, person("2"))
	if stamp() == written {
		t.Error("creating a second task left the stamp as it was")
	}
}
