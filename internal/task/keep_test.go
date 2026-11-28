package task_test

import (
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

func releasedRows(h *harness, id domain.TaskID) int {
	n := 0
	for _, r := range h.mem.Events(id) {
		if r.Kind == domain.KindNote && string(r.Data) == `{"text":"`+domain.NoteText(domain.PhaseReleased, "keep window ended")+`"}` {
			n++
		}
	}
	return n
}

func TestReleasedRecordsOnceAndOnlyAfterTheKeepWindowOfADoneOrFailedTask(t *testing.T) {
	for _, end := range []domain.State{domain.Done, domain.Failed} {
		t.Run(end.String(), func(t *testing.T) { releasedOnceAfterTheWindow(t, end) })
	}
}

func releasedOnceAfterTheWindow(t *testing.T, end domain.State) {
	h := newHarness(t)
	id := h.reach(domain.Plain, end)
	h.advance(cfg.KeepWindow - time.Second)
	if _, err := h.svc.Released(h.ctx(), id); !refused(err) {
		t.Fatalf("a release inside the window: err = %v", err)
	}
	h.advance(time.Second)
	if out := h.ok(h.svc.Released(h.ctx(), id)); len(out.Events) != 1 || releasedRows(h, id) != 1 {
		t.Fatalf("the release wrote %d rows, the task has %d release rows", len(out.Events), releasedRows(h, id))
	}
	if again := h.ok(h.svc.Released(h.ctx(), id)); len(again.Events) != 0 || releasedRows(h, id) != 1 {
		t.Error("a repeat of the release wrote rows")
	}
	if got := h.get(id); got.State != end {
		t.Errorf("the release moved the task to %s", got.State)
	}
}

func TestAReopenInsideTheWindowRestartsTheClock(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Done)
	h.advance(cfg.KeepWindow / 2)
	h.ok(h.svc.Steer(h.ctx(), id))
	if _, err := h.svc.Released(h.ctx(), id); !refused(err) {
		t.Fatalf("a reopened task is released: err = %v", err)
	}
	h.publish(id, "d2")
	h.advance(cfg.KeepWindow - time.Second)
	if _, err := h.svc.Released(h.ctx(), id); !refused(err) {
		t.Fatalf("the new window is shorter than the window: err = %v", err)
	}
	h.advance(time.Second)
	h.ok(h.svc.Released(h.ctx(), id))
	if releasedRows(h, id) != 1 {
		t.Errorf("%d release rows, want 1", releasedRows(h, id))
	}
}

func TestReleasedRefusesATaskThatIsNotEnded(t *testing.T) {
	for _, st := range []domain.State{domain.Waiting, domain.Starting, domain.Running, domain.Delivering, domain.Cancelled, domain.Superseded} {
		h := newHarness(t)
		id := h.reach(domain.Answering, st)
		h.advance(cfg.KeepWindow)
		if _, err := h.svc.Released(h.ctx(), id); !refused(err) {
			t.Errorf("%s: err = %v, want a refusal", st, err)
		}
	}
}
