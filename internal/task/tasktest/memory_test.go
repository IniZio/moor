package tasktest_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task/tasktest"
)

func newTask(id domain.TaskID, rev domain.Revision) domain.Task {
	sp := &domain.Spec{Name: "review", File: "specs/review.yaml"}
	ev := &domain.Event{Source: "forge", Group: "g", Rev: rev, Context: []byte("{}")}
	return domain.Task{ID: id, Project: "shop", Spec: sp, Event: ev, State: domain.Waiting, LastSeq: 1}
}

func row(id domain.TaskID, seq int64) domain.EventRow {
	return domain.EventRow{Task: id, Seq: seq, Kind: domain.KindState}
}

func TestMemoryEnforcesWhatADatabaseWould(t *testing.T) {
	ctx := context.Background()
	m := tasktest.NewMemory()
	first := newTask("t1", "r1")
	saved, err := m.Save(ctx, domain.TaskWrite{Task: first, Create: true, Events: []domain.EventRow{row("t1", 1)}})
	if err != nil || saved.Version != 1 {
		t.Fatalf("create: %+v, %v", saved, err)
	}

	cases := map[string]struct {
		w    domain.TaskWrite
		want error
	}{
		"same id":     {domain.TaskWrite{Task: first, Create: true, Events: []domain.EventRow{row("t1", 1)}}, domain.ErrExists},
		"same key":    {domain.TaskWrite{Task: newTask("t2", "r1"), Create: true, Events: []domain.EventRow{row("t2", 1)}}, domain.ErrExists},
		"stale":       {domain.TaskWrite{Task: first}, domain.ErrStaleVersion},
		"missing":     {domain.TaskWrite{Task: newTask("t9", "r9")}, domain.ErrNotFound},
		"duplicate N": {domain.TaskWrite{Task: saved, Result: &domain.ResultVersion{N: 2}}, domain.ErrExists},
	}
	for name, c := range cases {
		if _, err := m.Save(ctx, c.w); !errors.Is(err, c.want) {
			t.Errorf("%s: err = %v, want %v", name, err, c.want)
		}
	}

	gap := saved
	gap.LastSeq = 3
	if _, err := m.Save(ctx, domain.TaskWrite{Task: gap, Events: []domain.EventRow{row("t1", 3)}}); err == nil {
		t.Error("a seq gap was stored")
	}
	short := saved
	short.LastSeq = 5
	if _, err := m.Save(ctx, domain.TaskWrite{Task: short, Events: []domain.EventRow{row("t1", 2)}}); err == nil {
		t.Error("a last_seq that does not match the rows was stored")
	}
}

func TestMemoryVersionsAndGroup(t *testing.T) {
	ctx := context.Background()
	m := tasktest.NewMemory()
	a, b := newTask("a", "r1"), newTask("b", "r2")
	a.CreatedAt, b.CreatedAt = time.Unix(2, 0), time.Unix(1, 0)
	for _, tk := range []domain.Task{a, b} {
		if _, err := m.Save(ctx, domain.TaskWrite{Task: tk, Create: true, Events: []domain.EventRow{row(tk.ID, 1)}}); err != nil {
			t.Fatal(err)
		}
	}
	live, err := m.LiveInGroup(ctx, "shop", "review", "forge", "g")
	if err != nil || len(live) != 2 || live[0].ID != "b" {
		t.Fatalf("live = %+v, %v", live, err)
	}
	stored, _ := m.Load(ctx, "a")
	stored.State = domain.Done
	if _, err := m.Save(ctx, domain.TaskWrite{Task: stored, Result: &domain.ResultVersion{Task: "a", N: 1, Digest: "d"}}); err != nil {
		t.Fatal(err)
	}
	if v, _ := m.Versions(ctx, "a"); len(v) != 1 {
		t.Errorf("versions = %v", v)
	}
	if live, _ = m.LiveInGroup(ctx, "shop", "review", "forge", "g"); len(live) != 1 {
		t.Errorf("a done task is live: %d", len(live))
	}
	if _, err := m.ByKey(ctx, domain.TaskKey{Project: "shop", Spec: "review", Source: "forge", Group: "g", Rev: "nope"}); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("err = %v", err)
	}
}
