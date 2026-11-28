package task_test

import (
	"errors"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func TestReportFencesOlderRuns(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	before := h.get(id)
	_, err := h.svc.Report(h.ctx(), id, task.Report{Kind: task.HarnessExited, Run: before.Run - 1})
	if !errors.Is(err, task.ErrStaleReport) || h.get(id).State != domain.Running {
		t.Fatalf("err = %v, state %s", err, h.get(id).State)
	}
}

func TestReportBadInput(t *testing.T) {
	h := newHarness(t)
	id := h.person()
	for name, r := range map[string]task.Report{
		"no kind":        {},
		"slot no runner": {Kind: task.SlotTaken},
	} {
		if _, err := h.svc.Report(h.ctx(), id, r); !errors.Is(err, task.ErrBadInput) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	if _, err := h.svc.Report(h.ctx(), "missing", task.Report{Kind: task.SlotTaken, Runner: "r1"}); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("missing task: err = %v", err)
	}
}

func TestSlotTakenStartsARun(t *testing.T) {
	h := newHarness(t)
	id := h.person()
	h.slot(id)
	got := h.get(id)
	if got.State != domain.Starting || got.Runner != "r1" || got.Run != 1 {
		t.Fatalf("task = %+v", got)
	}
	if _, err := h.report(id, task.Report{Kind: task.SlotTaken, Runner: "r2"}); !refused(err) {
		t.Errorf("a second runner for the same task: err = %v", err)
	}
}

func TestPartsUpSetsTheDeadlineAndPartFailureRecordsWhy(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	if got := h.get(id); got.Deadline != h.clock.Now().Add(cfg.TimeLimit) {
		t.Errorf("deadline = %v", got.Deadline)
	}
	failed := h.reach(domain.Plain, domain.Failed)
	got := h.get(failed)
	rows := h.mem.Events(failed)
	if got.Failure != "no box" || rows[len(rows)-1].Kind != domain.KindError || got.KeepUntil.IsZero() {
		t.Errorf("failure %q, last row %s, keep until %v", got.Failure, rows[len(rows)-1].Kind, got.KeepUntil)
	}
}

func TestBoxRecreatedRaisesGenOnce(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	out := h.ok(h.report(id, task.Report{Kind: task.BoxRecreated, Gen: 1, HomeCommit: "abc"}))
	if got := h.get(id); got.Gen != 1 || got.State != domain.Running || out.Events[0].Kind != domain.KindBox {
		t.Fatalf("task = %+v", got)
	}
	if again := h.ok(h.report(id, task.Report{Kind: task.BoxRecreated, Gen: 1})); len(again.Events) != 0 {
		t.Error("a repeat wrote events")
	}
	if older := h.ok(h.report(id, task.Report{Kind: task.BoxRecreated, Gen: 0})); len(older.Events) != 0 || h.get(id).Gen != 1 {
		t.Error("an older generation changed the task")
	}
}

func TestTimeLimit(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	h.advance(cfg.TimeLimit - time.Second)
	if out := h.ok(h.svc.TimeLimit(h.ctx(), id)); len(out.Events) != 0 || h.get(id).State != domain.Running {
		t.Fatal("the limit fired early")
	}
	h.advance(time.Second)
	out := h.ok(h.svc.TimeLimit(h.ctx(), id))
	if got := h.get(id); got.State != domain.Failed || got.Failure != "time limit reached" || len(out.Events) != 2 {
		t.Fatalf("task = %+v", got)
	}
}

func TestSteerReopensAndRestartsTheLimit(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Done)
	h.advance(time.Hour)
	h.ok(h.svc.Steer(h.ctx(), id))
	got := h.get(id)
	if got.State != domain.Running || !got.EndedAt.IsZero() || !got.KeepUntil.IsZero() || got.Deadline != h.clock.Now().Add(cfg.TimeLimit) {
		t.Fatalf("task = %+v", got)
	}
}

func TestSteerAfterTheKeepWindowIsRefused(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Done)
	h.advance(cfg.KeepWindow)
	if _, err := h.svc.Steer(h.ctx(), id); !refused(err) || h.get(id).State != domain.Done {
		t.Fatalf("err = %v, state %s", err, h.get(id).State)
	}
}

func TestNoLimitsWhenConfiguredZero(t *testing.T) {
	h := newHarness(t)
	svc := task.New(h.mem, h.clock, task.Config{})
	person := h.id("zero")
	if _, err := svc.StartByPerson(h.ctx(), task.StartInput{ID: person, Repo: h.repo, Prompt: "p", By: "ann"}); err != nil {
		t.Fatal(err)
	}
	for _, r := range []task.Report{{Kind: task.SlotTaken, Runner: "r1"}, {Kind: task.PartsUp, Run: 1}} {
		if _, err := svc.Report(h.ctx(), person, r); err != nil {
			t.Fatal(err)
		}
	}
	h.advance(1000 * time.Hour)
	if out, err := svc.TimeLimit(h.ctx(), person); err != nil || len(out.Events) != 0 {
		t.Errorf("a task with no deadline hit its limit: %v", err)
	}
}

func TestCancelAndEndReportEvents(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	out := h.ok(h.svc.Cancel(h.ctx(), id))
	if len(out.Effects) != 1 || out.Effects[0].Kind != task.StopJob || out.Effects[0].Runner != "r1" {
		t.Fatalf("effects = %+v", out.Effects)
	}
	if again := h.ok(h.svc.Cancel(h.ctx(), id)); len(again.Events) != 0 || len(again.Effects) != 1 {
		t.Errorf("a repeated cancel: %d events, %d effects", len(again.Events), len(again.Effects))
	}
	waiting := h.reach(domain.Answering, domain.Waiting)
	if out := h.ok(h.svc.Cancel(h.ctx(), waiting)); len(out.Effects) != 0 {
		t.Errorf("a waiting task has no job to stop: %+v", out.Effects)
	}
	person := h.reach(domain.Plain, domain.Running)
	h.ok(h.svc.End(h.ctx(), person))
	if h.get(person).State != domain.Done {
		t.Error("end did not finish the task")
	}
}
