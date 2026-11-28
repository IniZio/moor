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

const source domain.TriggerSource = "forge"

var reply = domain.ReplyTarget{Source: "forge", Ref: "acme/app#7"}

// The four shapes of task the state machine tells apart. personFlow and triggerFlow are the ones
// most tests drive, since they are what a person's prompt and a trigger's event usually start.
var (
	personFlow    = domain.Flow{}
	personReplies = domain.Flow{Replies: true}
	triggerSilent = domain.Flow{Triggered: true}
	triggerFlow   = domain.Flow{Triggered: true, Replies: true}
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
	clock := tasktest.NewClock(time.Date(2026, 11, 28, 9, 0, 0, 0, time.UTC))
	var store task.Store = mem
	if wrap != nil {
		store = wrap(mem)
	}
	return &harness{t: t, mem: mem, clock: clock, svc: task.New(store, clock, cfg), proj: proj}
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

func (h *harness) person() domain.TaskID {
	id := h.id("person")
	h.ok(h.svc.StartByPerson(h.ctx(), task.StartInput{ID: id, Project: h.proj, Prompt: "look at it", By: "ann"}))
	return id
}

// trigIn is an event that starts a task whose result is posted to reply.
func (h *harness) trigIn(id domain.TaskID, group, rev string) task.TriggerInput {
	tr := domain.Trigger{Name: "pr-evidence", Source: source, Group: domain.TriggerGroup(group), Rev: domain.Revision(rev), Context: []byte(`{"pr":7}`)}
	return task.TriggerInput{ID: id, Project: h.proj, Prompt: "show it", Reply: reply, Trigger: tr}
}

func (h *harness) trig(group, rev string) domain.TaskID {
	id := h.id("trig")
	h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn(id, group, rev)))
	return id
}

// start makes a waiting task of the given shape.
func (h *harness) start(flow domain.Flow, review string) domain.TaskID {
	h.t.Helper()
	if flow.Triggered {
		id := h.id("trig")
		in := h.trigIn(id, fmt.Sprintf("acme/app#%d", h.n+1), "rev-1")
		in.ReviewPrompt = review
		if !flow.Replies {
			in.Reply = domain.ReplyTarget{}
		}
		h.ok(h.svc.StartByTrigger(h.ctx(), in))
		return id
	}
	id := h.id("person")
	in := task.StartInput{ID: id, Project: h.proj, Prompt: "look at it", ReviewPrompt: review, By: "ann"}
	if flow.Replies {
		in.Reply = reply
	}
	h.ok(h.svc.StartByPerson(h.ctx(), in))
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
	id := h.start(flow, "")
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
		if flow.Replies {
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
		h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn(h.id("newer"), string(g), "rev-2")))
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
