package task_test

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
)

const (
	plain  domain.RecipeName    = "pr-plain"
	review domain.RecipeName    = "pr-review"
	source domain.TriggerSource = "forge"
)

var cfg = task.Config{TimeLimit: time.Hour, KeepWindow: 24 * time.Hour}

type harness struct {
	t     *testing.T
	mem   *tasktest.Memory
	clock *tasktest.Clock
	svc   *task.Service
	proj  domain.Project
	n     int
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	return newHarnessOn(t, tasktest.NewMemory(), nil)
}

// newHarnessOn builds a harness whose service reads and writes through wrap(mem), when given.
func newHarnessOn(t *testing.T, mem *tasktest.Memory, wrap func(task.Store) task.Store) *harness {
	t.Helper()
	entry := domain.Entry{Repo: domain.Repo{Host: "forge.example", Path: "acme/app"}, Ref: "main", Dir: "app"}
	proj, err := domain.NewProject("shop", []domain.Entry{entry})
	if err != nil {
		t.Fatal(err)
	}
	recipes := map[domain.RecipeName]domain.Recipe{
		plain:  {Name: plain, Prompt: "show it", Source: source},
		review: {Name: review, Prompt: "show it, then review", Source: source, ReviewTurn: true},
	}
	lookup := func(n domain.RecipeName) (domain.Recipe, bool) { r, ok := recipes[n]; return r, ok }
	clock := tasktest.NewClock(time.Date(2026, 11, 28, 9, 0, 0, 0, time.UTC))
	var store task.Store = mem
	if wrap != nil {
		store = wrap(mem)
	}
	return &harness{t: t, mem: mem, clock: clock, svc: task.New(store, clock, lookup, cfg), proj: proj}
}

func (h *harness) id(prefix string) domain.TaskID {
	h.n++
	return domain.TaskID(fmt.Sprintf("%s-%d", prefix, h.n))
}

func (h *harness) ctx() context.Context { return context.Background() }

func (h *harness) ok(out task.Outcome, err error) task.Outcome {
	h.t.Helper()
	if err != nil {
		h.t.Fatalf("unexpected error: %v", err)
	}
	return out
}

func (h *harness) free() domain.TaskID {
	id := h.id("free")
	h.ok(h.svc.StartByPerson(h.ctx(), task.StartInput{ID: id, Project: h.proj, Prompt: "look at it", By: "ann"}))
	return id
}

func (h *harness) trigIn(id domain.TaskID, group, rev string, recipe domain.RecipeName) task.TriggerInput {
	r := domain.Recipe{Name: recipe, Prompt: "show it", Source: source}
	tr := domain.Trigger{Source: source, Group: domain.TriggerGroup(group), Rev: domain.Revision(rev), Context: []byte(`{"pr":7}`)}
	return task.TriggerInput{ID: id, Project: h.proj, Recipe: r, Trigger: tr}
}

func (h *harness) trig(group, rev string, recipe domain.RecipeName) domain.TaskID {
	id := h.id("trig")
	h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn(id, group, rev, recipe)))
	return id
}

func (h *harness) get(id domain.TaskID) domain.Task {
	h.t.Helper()
	got, err := h.mem.Load(h.ctx(), id)
	if err != nil {
		h.t.Fatal(err)
	}
	return got
}

func (h *harness) report(id domain.TaskID, r task.Report) (task.Outcome, error) {
	r.Run = h.get(id).Run
	if r.Runner == "" {
		r.Runner = "r1"
	}
	return wrapped(h.svc.Report(h.ctx(), id, r))
}

func (h *harness) slot(id domain.TaskID) { h.ok(h.report(id, task.Report{Kind: task.SlotTaken})) }

func (h *harness) up(id domain.TaskID) { h.ok(h.report(id, task.Report{Kind: task.PartsUp})) }

func (h *harness) running(id domain.TaskID) { h.slot(id); h.up(id) }

func (h *harness) publish(id domain.TaskID, digest string) task.Outcome {
	return h.ok(h.svc.Publish(h.ctx(), id, task.PublishInput{Line: "it works", EntryURL: "https://pages.example/" + digest, Digest: digest}))
}

func (h *harness) advance(d time.Duration) { h.clock.Advance(d) }

var allStates = []domain.State{
	domain.Waiting, domain.Starting, domain.Running, domain.Delivering,
	domain.Done, domain.Failed, domain.Superseded, domain.Cancelled,
}

// reach drives a new task into a state through the operations a real run would use.
func (h *harness) reach(flow domain.Flow, want domain.State) domain.TaskID {
	h.t.Helper()
	var id domain.TaskID
	if flow == domain.Triggered {
		id = h.trig(fmt.Sprintf("acme/app#%d", h.n+1), "rev-1", plain)
	} else {
		id = h.free()
	}
	steps[want](h, flow, id)
	if got := h.get(id).State; got != want {
		h.t.Fatalf("reach %s %s: got %s", flow, want, got)
	}
	return id
}

var steps = map[domain.State]func(*harness, domain.Flow, domain.TaskID){
	domain.Waiting:  func(*harness, domain.Flow, domain.TaskID) {},
	domain.Starting: func(h *harness, _ domain.Flow, id domain.TaskID) { h.slot(id) },
	domain.Running:  func(h *harness, _ domain.Flow, id domain.TaskID) { h.running(id) },
	domain.Delivering: func(h *harness, _ domain.Flow, id domain.TaskID) {
		h.running(id)
		h.publish(id, "d1")
	},
	domain.Done: func(h *harness, flow domain.Flow, id domain.TaskID) {
		h.running(id)
		h.publish(id, "d1")
		if flow == domain.Triggered {
			h.ok(h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Target: "pr", ExternalID: "c1", Version: 1}))
		}
	},
	domain.Failed: func(h *harness, _ domain.Flow, id domain.TaskID) {
		h.slot(id)
		h.ok(h.report(id, task.Report{Kind: task.PartFailed, Text: "no box"}))
	},
	domain.Superseded: func(h *harness, _ domain.Flow, id domain.TaskID) {
		h.running(id)
		g := h.get(id).Trigger.Group
		h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn(h.id("newer"), string(g), "rev-2", plain)))
	},
	domain.Cancelled: func(h *harness, _ domain.Flow, id domain.TaskID) {
		h.running(id)
		h.ok(h.svc.Cancel(h.ctx(), id))
	},
}

func kinds(rows []domain.EventRow) []domain.EventKind {
	out := make([]domain.EventKind, len(rows))
	for i, r := range rows {
		out[i] = r.Kind
	}
	return out
}

func refused(err error) bool { return errors.Is(err, domain.ErrRefused) }

// wrapped and errOf only add context to an error, which the wrapcheck rule asks of every caller.
func wrapped(out task.Outcome, err error) (task.Outcome, error) {
	if err != nil {
		return out, fmt.Errorf("service call: %w", err)
	}
	return out, nil
}

func errOf(_ task.Outcome, err error) error {
	if err != nil {
		return fmt.Errorf("service call: %w", err)
	}
	return nil
}
