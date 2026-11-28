package storetest

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func roundTrip(t *testing.T, s Subject) {
	ctx := context.Background()
	in := full("t1")
	saved, err := s.Store.Save(ctx, domain.TaskWrite{Task: in, Create: true, Events: eventsFrom("t1", 1, 2)})
	if err != nil {
		t.Fatal(err)
	}
	want := in
	want.Version = 1
	sameTask(t, "saved", saved, want)
	sameTask(t, "loaded", load(t, s, "t1"), want)
	key, _ := in.Key()
	byKey, err := s.Store.ByKey(ctx, key)
	if err != nil {
		t.Fatal(err)
	}
	sameTask(t, "by key", byKey, want)
	sameEvents(t, "events", events(t, s, "t1"), eventsFrom("t1", 1, 2))
}

func freePrompt(t *testing.T, s Subject) {
	a, b := create(t, s, free("a")), create(t, s, free("b"))
	if a.Trigger != nil || b.Trigger != nil || a.Flow() != domain.FreePrompt {
		t.Errorf("a free-prompt task came back with a trigger: %+v", a.Trigger)
	}
	if _, ok := load(t, s, "a").Key(); ok {
		t.Error("a free-prompt task has a key")
	}
}

func missing(t *testing.T, s Subject) {
	ctx := context.Background()
	_, err := s.Store.Load(ctx, "nope")
	wantErr(t, "load", err, domain.ErrNotFound)
	_, err = s.Store.ByKey(ctx, domain.TaskKey{Project: "shop", Source: "forge", Group: "g", Rev: "nope"})
	wantErr(t, "by key", err, domain.ErrNotFound)
	_, err = s.Store.Save(ctx, domain.TaskWrite{Task: free("nope")})
	wantErr(t, "save", err, domain.ErrNotFound)
	if v, err := s.Store.Versions(ctx, "nope"); err != nil || len(v) != 0 {
		t.Errorf("versions of no task = %v, %v", v, err)
	}
}

func duplicates(t *testing.T, s Subject) {
	ctx := context.Background()
	create(t, s, triggered("t1", "r1"))
	cases := map[string]domain.Task{"same id": triggered("t1", "r2"), "same key": triggered("t2", "r1")}
	for name, task := range cases {
		_, err := s.Store.Save(ctx, domain.TaskWrite{Task: task, Create: true, Events: eventsFrom(task.ID, 1, 1)})
		wantErr(t, name, err, domain.ErrExists)
	}
	other := triggered("t3", "r1")
	other.Project = "blog"
	create(t, s, other)
	if _, err := s.Store.Load(ctx, "t2"); err == nil {
		t.Error("a refused create left a task behind")
	}
	if rows, _ := s.Events(ctx, "t2"); len(rows) != 0 {
		t.Errorf("a refused create left %d events", len(rows))
	}
}

func staleVersion(t *testing.T, s Subject) {
	ctx := context.Background()
	first := create(t, s, free("t1"))
	next := first
	next.State, next.LastSeq = domain.Starting, 2
	if _, err := s.Store.Save(ctx, domain.TaskWrite{Task: next, Events: eventsFrom("t1", 2, 2)}); err != nil {
		t.Fatal(err)
	}
	late := first
	late.State, late.LastSeq = domain.Cancelled, 2
	_, err := s.Store.Save(ctx, domain.TaskWrite{Task: late, Events: eventsFrom("t1", 2, 2)})
	wantErr(t, "late writer", err, domain.ErrStaleVersion)
	if got := load(t, s, "t1"); got.State != domain.Starting || got.Version != 2 {
		t.Errorf("stored %v at version %d", got.State, got.Version)
	}
	if rows := events(t, s, "t1"); len(rows) != 2 {
		t.Errorf("%d events, want 2", len(rows))
	}
}

func versionBumps(t *testing.T, s Subject) {
	cur := create(t, s, free("t1"))
	for want := int64(2); want <= 4; want++ {
		saved, err := s.Store.Save(context.Background(), domain.TaskWrite{Task: cur})
		if err != nil || saved.Version != want {
			t.Fatalf("version = %d, %v, want %d", saved.Version, err, want)
		}
		cur = saved
	}
}

func badEvents(t *testing.T, s Subject) {
	ctx := context.Background()
	cur := create(t, s, free("t1"))
	gap, short, foreign := cur, cur, cur
	gap.LastSeq, short.LastSeq, foreign.LastSeq = 3, 5, 2
	other := event("t9", 2)
	cases := map[string]domain.TaskWrite{
		"a seq gap":        {Task: gap, Events: eventsFrom("t1", 3, 3)},
		"last_seq too far": {Task: short, Events: eventsFrom("t1", 2, 2)},
		"another task":     {Task: foreign, Events: []domain.EventRow{other}},
		"a repeated seq":   {Task: cur, Events: eventsFrom("t1", 1, 1)},
	}
	for name, w := range cases {
		if _, err := s.Store.Save(ctx, w); err == nil {
			t.Errorf("%s was stored", name)
		}
	}
	if got := load(t, s, "t1"); got.Version != 1 || got.LastSeq != 1 {
		t.Errorf("a refused write changed the task: %+v", got)
	}
	sameEvents(t, "events", events(t, s, "t1"), eventsFrom("t1", 1, 1))
}

func resultVersions(t *testing.T, s Subject) {
	ctx := context.Background()
	cur := create(t, s, free("t1"))
	for n := 1; n <= 3; n++ {
		r := domain.ResultVersion{N: n, EntryURL: "u", Line: "l", Digest: string(rune('a' + n)), PublishedAt: at(int64(n))}
		saved, err := s.Store.Save(ctx, domain.TaskWrite{Task: cur, Result: &r})
		if err != nil {
			t.Fatal(err)
		}
		cur = saved
	}
	got, err := s.Store.Versions(ctx, "t1")
	if err != nil || len(got) != 3 {
		t.Fatalf("versions = %v, %v", got, err)
	}
	for i, v := range got {
		if v.N != i+1 || v.Task != "t1" || v.EntryURL != "u" || v.Line != "l" || !v.PublishedAt.Equal(at(int64(i+1))) {
			t.Errorf("version %d = %+v", i, v)
		}
	}
	wrong := domain.ResultVersion{N: 9, Digest: "z"}
	_, err = s.Store.Save(ctx, domain.TaskWrite{Task: cur, Result: &wrong})
	wantErr(t, "a gap in N", err, domain.ErrExists)
}

func refusedResultUndoesWrite(t *testing.T, s Subject) {
	ctx := context.Background()
	cur := create(t, s, free("t1"))
	first := domain.ResultVersion{N: 1, Digest: "d"}
	cur, err := s.Store.Save(ctx, domain.TaskWrite{Task: cur, Result: &first})
	if err != nil {
		t.Fatal(err)
	}
	again := cur
	again.State, again.LastSeq = domain.Done, 2
	repeat := domain.ResultVersion{N: 2, Digest: "d"}
	_, err = s.Store.Save(ctx, domain.TaskWrite{Task: again, Events: eventsFrom("t1", 2, 2), Result: &repeat})
	wantErr(t, "the same digest", err, domain.ErrExists)
	if got := load(t, s, "t1"); got.State != domain.Waiting || got.Version != cur.Version || got.LastSeq != 1 {
		t.Errorf("a refused result left %+v", got)
	}
	if rows := events(t, s, "t1"); len(rows) != 1 {
		t.Errorf("a refused result left %d events", len(rows))
	}
	if v, _ := s.Store.Versions(ctx, "t1"); len(v) != 1 {
		t.Errorf("%d versions, want 1", len(v))
	}
}

func liveInGroup(t *testing.T, s Subject) {
	mk := func(id domain.TaskID, rev domain.Revision, created int64, state domain.State, edit func(*domain.Task)) {
		task := triggered(id, rev)
		task.CreatedAt, task.State = at(created), state
		if edit != nil {
			edit(&task)
		}
		create(t, s, task)
	}
	mk("a", "r1", 2, domain.Waiting, nil)
	mk("b", "r2", 1, domain.Running, nil)
	mk("c", "r3", 3, domain.Done, nil)
	mk("d", "r4", 4, domain.Delivering, func(x *domain.Task) { x.Trigger.Group = "other" })
	mk("e", "r5", 5, domain.Waiting, func(x *domain.Task) { x.Project = "blog" })
	mk("f", "r6", 6, domain.Waiting, func(x *domain.Task) { x.Trigger.Source = "other" })
	mk("g", "r7", 7, domain.Superseded, nil)
	create(t, s, free("h"))
	got, err := s.Store.LiveInGroup(context.Background(), "shop", "forge", "g")
	if err != nil || len(got) != 2 || got[0].ID != "b" || got[1].ID != "a" {
		t.Errorf("live = %+v, %v, want b then a", got, err)
	}
}

func copies(t *testing.T, s Subject) {
	create(t, s, full("t1"))
	got := load(t, s, "t1")
	got.Pins["api"] = "changed"
	got.Trigger.Context[0] = 'X'
	again := load(t, s, "t1")
	if again.Pins["api"] != "abc" || again.Trigger.Context[0] != '{' {
		t.Errorf("a caller changed the stored task: %+v", again)
	}
}
