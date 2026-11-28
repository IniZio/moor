package task_test

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
)

var errBoom = errors.New("boom")

// faultStore breaks Save on demand: before runs just ahead of a save, and failAt fails the nth one.
type faultStore struct {
	task.Store
	saves  int
	failAt int
	before func()
	stale  bool
}

func (f *faultStore) Save(ctx context.Context, w domain.TaskWrite) (domain.Task, error) {
	f.saves++
	if f.before != nil {
		hook := f.before
		f.before = nil
		hook()
	}
	if f.saves == f.failAt {
		return domain.Task{}, errBoom
	}
	if f.stale {
		return domain.Task{}, fmt.Errorf("always: %w", domain.ErrStaleVersion)
	}
	saved, err := f.Store.Save(ctx, w)
	if err != nil {
		return domain.Task{}, fmt.Errorf("save: %w", err)
	}
	return saved, nil
}

func faulty(t *testing.T) (*harness, *faultStore) {
	t.Helper()
	var fs *faultStore
	h := newHarnessOn(t, tasktest.NewMemory(), func(s task.Store) task.Store {
		fs = &faultStore{Store: s}
		return fs
	})
	return h, fs
}

func TestStaleWriteIsRefusedThenRetried(t *testing.T) {
	h, fs := faulty(t)
	id := h.reach(triggerFlow, domain.Running)
	other := task.New(h.mem, h.clock, cfg)
	fs.before = func() {
		if _, err := other.Report(h.ctx(), id, task.Report{Kind: task.BoxRecreated, Run: 1, Gen: 1}); err != nil {
			t.Error(err)
		}
	}
	savesBefore := fs.saves
	out := h.ok(h.svc.Cancel(h.ctx(), id))
	got := h.get(id)
	if got.State != domain.Cancelled || got.Gen != 1 || out.Task.Version != got.Version {
		t.Fatalf("task = %+v", got)
	}
	if fs.saves-savesBefore != 2 {
		t.Errorf("saves = %d, want the refused one and the retry", fs.saves-savesBefore)
	}
	if last := h.mem.Events(id); last[len(last)-1].Seq != int64(len(last)) {
		t.Errorf("seq has a gap after the race: %d events, last seq %d", len(last), last[len(last)-1].Seq)
	}
}

func TestStaleWritesGiveUp(t *testing.T) {
	h, fs := faulty(t)
	id := h.reach(personFlow, domain.Running)
	fs.stale = true
	fs.saves = 0
	before := h.get(id)
	_, err := h.svc.Cancel(h.ctx(), id)
	if !errors.Is(err, domain.ErrStaleVersion) || fs.saves != 5 {
		t.Fatalf("err = %v after %d saves", err, fs.saves)
	}
	if after := h.get(id); after.State != before.State || after.Version != before.Version {
		t.Error("a task changed though every save was refused")
	}
}

func TestCreateRacingOnTheSameKeyFindsTheWinner(t *testing.T) {
	h, fs := faulty(t)
	other := task.New(h.mem, h.clock, cfg)
	fs.before = func() {
		if _, err := other.StartByTrigger(h.ctx(), h.trigIn("winner", "acme/app#7", "abc")); err != nil {
			t.Error(err)
		}
	}
	out := h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn("loser", "acme/app#7", "abc")))
	if out.Task.ID != "winner" || len(h.mem.Tasks()) != 1 {
		t.Errorf("task %s, %d tasks", out.Task.ID, len(h.mem.Tasks()))
	}
}

func TestStorageErrorsPass(t *testing.T) {
	h, fs := faulty(t)
	fs.failAt = 1
	if _, err := h.svc.StartByPerson(h.ctx(), task.StartInput{ID: "t1", Project: h.proj, Prompt: "p", By: "ann"}); !errors.Is(err, errBoom) {
		t.Fatalf("err = %v", err)
	}
	if len(h.mem.Tasks()) != 0 {
		t.Error("a failed save left a task")
	}
	if _, err := h.svc.StartByPerson(h.ctx(), task.StartInput{ID: "t1", Project: h.proj, Prompt: "p", By: "ann"}); err != nil {
		t.Fatalf("the retry after the failure: %v", err)
	}
}
