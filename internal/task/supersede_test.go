package task_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func newerRevOf(h *harness, old domain.TaskID, rev string) task.Outcome {
	return h.ok(h.svc.StartByEvent(h.ctx(), h.newerIn(h.id("newer"), old, rev)))
}

func TestNewerRevSupersedesLiveTasks(t *testing.T) {
	for _, state := range []domain.State{domain.Waiting, domain.Starting, domain.Running, domain.Delivering} {
		t.Run(state.String(), func(t *testing.T) {
			h := newHarness(t)
			old := h.reach(domain.Answering, state)
			out := newerRevOf(h, old, "rev-2")
			checkSuperseded(h, old, out)
			if got, want := hasStop(out.Effects, old), state != domain.Waiting; got != want {
				t.Errorf("stop job for %s = %v, want %v", old, got, want)
			}
		})
	}
}

func checkSuperseded(h *harness, old domain.TaskID, out task.Outcome) {
	h.t.Helper()
	got := h.get(old)
	if got.State != domain.Superseded || got.EndedAt.IsZero() {
		h.t.Fatalf("old task = %+v", got)
	}
	if out.Task.State != domain.Waiting || out.Task.Event.Rev != "rev-2" || !out.Task.CreatedAt.After(got.CreatedAt) {
		h.t.Fatalf("new task = %+v, which must also be newer than the old one on the same clock tick", out.Task)
	}
	if len(out.Events) != 2 || out.Events[1].Task != old {
		h.t.Errorf("events = %v, want the new task's row and the old task's", kinds(out.Events))
	}
}

func hasStop(fx []task.Effect, id domain.TaskID) bool {
	for _, e := range fx {
		if e.Kind == task.StopJob && e.Task == id && e.Runner == "r1" {
			return true
		}
	}
	return false
}

func TestSupersedeKeepsDeliveringVersionReadable(t *testing.T) {
	h := newHarness(t)
	old := h.reach(domain.Answering, domain.Delivering)
	newerRevOf(h, old, "rev-2")
	versions, _ := h.mem.Versions(h.ctx(), old)
	if len(versions) != 1 {
		t.Errorf("versions = %d", len(versions))
	}
	_, err := h.svc.Delivered(h.ctx(), old, task.DeliveredInput{Version: 1})
	if !refused(err) {
		t.Errorf("a delivery for a superseded task: err = %v", err)
	}
}

func TestSupersedeLeavesOthersAlone(t *testing.T) {
	for _, state := range []domain.State{domain.Done, domain.Failed, domain.Cancelled} {
		h := newHarness(t)
		old := h.reach(domain.Answering, state)
		out := newerRevOf(h, old, "rev-2")
		if got := h.get(old).State; got != state || len(out.Effects) != 0 || len(out.Events) != 1 {
			t.Errorf("%s: old task now %s, effects %v, events %d", state, got, out.Effects, len(out.Events))
		}
	}

	h := newHarness(t)
	other := h.ev("acme/app#1", "rev-1")
	h.ev("acme/app#2", "rev-1")
	in := h.evIn("blog-task", "acme/app#1", "rev-2")
	in.Repo = blog
	h.ok(h.svc.StartByEvent(h.ctx(), in))
	if got := h.get(other).State; got != domain.Waiting {
		t.Errorf("a task of another group or repository was superseded: %s", got)
	}
}

func TestNewerEventOfAnotherSpecSupersedesNothing(t *testing.T) {
	h := newHarness(t)
	old := h.ev("acme/app#1", "rev-1")
	in := h.evIn("nightly-1", "acme/app#1", "rev-2")
	in.Spec = nightly
	out := h.ok(h.svc.StartByEvent(h.ctx(), in))
	if got := h.get(old).State; got != domain.Waiting || len(out.Events) != 1 {
		t.Errorf("a task of another spec: old task %s, %d events", got, len(out.Events))
	}
}

func TestWithoutCancelInProgressANewerEventSupersedesNothing(t *testing.T) {
	for _, state := range []domain.State{domain.Waiting, domain.Starting, domain.Running, domain.Delivering} {
		t.Run(state.String(), func(t *testing.T) {
			h := newHarness(t)
			old := h.reach(domain.Answering, state)
			in := h.newerIn("newer", old, "rev-2")
			in.CancelInProgress = false
			out := h.ok(h.svc.StartByEvent(h.ctx(), in))
			if got := h.get(old).State; got != state || len(out.Effects) != 0 || len(out.Events) != 1 || h.get("newer").State != domain.Waiting {
				t.Errorf("old task now %s, effects %v, events %d", got, out.Effects, len(out.Events))
			}
		})
	}
}

func TestPersonStartedTasksAreNeverSuperseded(t *testing.T) {
	h := newHarness(t)
	person := h.reach(domain.Plain, domain.Running)
	h.ev("acme/app#1", "rev-1")
	h.ev("acme/app#1", "rev-2")
	if got := h.get(person).State; got != domain.Running {
		t.Errorf("person-started task = %s", got)
	}
}

func TestOlderRevRedeliveredAfterNewerStartsNothing(t *testing.T) {
	h := newHarness(t)
	first := h.ev("acme/app#1", "rev-1")
	second := h.ev("acme/app#1", "rev-2")
	out := h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("late", "acme/app#1", "rev-1")))
	if out.Task.ID != first || len(out.Events) != 0 {
		t.Fatalf("redelivery returned %s with %d events", out.Task.ID, len(out.Events))
	}
	if h.get(second).State != domain.Waiting || h.get(first).State != domain.Superseded {
		t.Errorf("states = %s, %s", h.get(first).State, h.get(second).State)
	}
}

func TestSupersedeSurvivesACrashBetweenTheWrites(t *testing.T) {
	h, fs := faulty(t)
	old := h.reach(domain.Answering, domain.Running)
	g := string(h.get(old).Event.Group)
	in := h.evIn("newer", g, "rev-2")

	fs.failAt = fs.saves + 2
	if _, err := h.svc.StartByEvent(h.ctx(), in); err == nil {
		t.Fatal("the injected failure did not reach the caller")
	}
	if h.get(old).State != domain.Running || h.get("newer").State != domain.Waiting {
		t.Fatal("expected the new task stored and the old one still live")
	}

	out := h.ok(h.svc.StartByEvent(h.ctx(), in))
	if h.get(old).State != domain.Superseded || !hasStop(out.Effects, old) || out.Task.ID != "newer" {
		t.Fatalf("redelivery did not finish the supersede: old %s, effects %v", h.get(old).State, out.Effects)
	}
	if n := len(h.mem.Events("newer")); n != 1 {
		t.Errorf("the new task has %d events", n)
	}
}
