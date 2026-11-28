package task_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestARevokedMachineFailsTheTasksItStartsOrRuns(t *testing.T) {
	h := newHarness(t)
	for _, from := range []domain.State{domain.Starting, domain.Running} {
		id := h.reach(domain.Plain, from)
		out := h.ok(h.svc.MachineRevoked(h.ctx(), id))
		if got := h.get(id); got.State != domain.Failed || got.Failure != "machine revoked" || len(out.Events) == 0 {
			t.Errorf("from %s: %s with failure %q", from, got.State, got.Failure)
		}
		if again := h.ok(h.svc.MachineRevoked(h.ctx(), id)); len(again.Events) != 0 {
			t.Errorf("from %s: a repeat recorded %d events", from, len(again.Events))
		}
	}
	for _, from := range []domain.State{domain.Waiting, domain.Done, domain.Cancelled} {
		id := h.reach(domain.Plain, from)
		if _, err := h.svc.MachineRevoked(h.ctx(), id); !refused(err) {
			t.Errorf("from %s: err = %v, want refused", from, err)
		}
	}
}
