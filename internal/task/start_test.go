package task_test

import (
	"errors"
	"fmt"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func TestStartByPerson(t *testing.T) {
	h := newHarness(t)
	in := task.StartInput{
		ID: "t1", Repo: h.repo, Prompt: "look at it", By: "ann", Ref: "feature",
	}
	out := h.ok(h.svc.StartByPerson(h.ctx(), in))
	got := out.Task
	if got.State != domain.Waiting || got.StartedBy != "ann" || got.Repo != app || got.Version != 1 || got.LastSeq != 1 || got.Ref != "feature" {
		t.Fatalf("task = %+v", got)
	}
	if rows := out.Events; len(rows) != 1 || rows[0].Kind != domain.KindState || rows[0].Seq != 1 || rows[0].Task != "t1" {
		t.Fatalf("events = %+v", rows)
	}

	again := h.ok(h.svc.StartByPerson(h.ctx(), in))
	if len(again.Events) != 0 || again.Task.ID != "t1" || len(h.mem.Tasks()) != 1 || len(h.mem.Events("t1")) != 1 {
		t.Fatalf("a repeated start made %d tasks, %d events", len(h.mem.Tasks()), len(h.mem.Events("t1")))
	}

	other := in
	other.Prompt = "something else"
	if _, err := h.svc.StartByPerson(h.ctx(), other); !errors.Is(err, task.ErrIDTaken) {
		t.Errorf("same id, other prompt: err = %v", err)
	}
}

func TestPersonMayStartManyTasks(t *testing.T) {
	h := newHarness(t)
	h.person()
	h.person()
	if n := len(h.mem.Tasks()); n != 2 {
		t.Errorf("tasks = %d", n)
	}
}

func TestStartByPersonKeepsTheReviewPrompt(t *testing.T) {
	h := newHarness(t)
	in := task.StartInput{ID: "t1", Repo: h.repo, Prompt: "p", ReviewPrompt: "check it", By: "ann"}
	got := h.ok(h.svc.StartByPerson(h.ctx(), in)).Task
	if got.ReviewPrompt != "check it" || got.Spec != nil || got.Event != nil || got.Flow(cfg.Answerable) != domain.Plain {
		t.Errorf("task = %+v", got)
	}
	if again := h.ok(h.svc.StartByPerson(h.ctx(), in)); len(again.Events) != 0 || len(h.mem.Tasks()) != 1 {
		t.Errorf("a repeated start made %d events, %d tasks", len(again.Events), len(h.mem.Tasks()))
	}
	other := in
	other.ReviewPrompt = "other"
	if _, err := h.svc.StartByPerson(h.ctx(), other); !errors.Is(err, task.ErrIDTaken) {
		t.Errorf("same id, other review prompt: err = %v", err)
	}
}

func TestStartByPersonRefusesBadInput(t *testing.T) {
	h := newHarness(t)
	ok := task.StartInput{ID: "t1", Repo: h.repo, Prompt: "p", By: "ann"}
	bad := map[string]func(*task.StartInput){
		"no id":     func(in *task.StartInput) { in.ID = "" },
		"no person": func(in *task.StartInput) { in.By = "" },
		"no host":   func(in *task.StartInput) { in.Repo.Host = "" },
		"no path":   func(in *task.StartInput) { in.Repo.Path = "" },
		"no prompt": func(in *task.StartInput) { in.Prompt = "" },
	}
	for name, mutate := range bad {
		in := ok
		mutate(&in)
		if _, err := h.svc.StartByPerson(h.ctx(), in); !errors.Is(err, task.ErrBadInput) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	if len(h.mem.Tasks()) != 0 {
		t.Error("a refused start stored a task")
	}
}

func TestStartByEventDedupesOnKey(t *testing.T) {
	h := newHarness(t)
	first := h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("t1", "acme/app#7", "abc")))
	got := first.Task
	if got.Flow(cfg.Answerable) != domain.Answering || got.Prompt != "show it" || got.StartedBy != "event:forge" {
		t.Fatalf("task = %+v", got)
	}
	if got.Spec == nil || *got.Spec != spec || got.Event == nil || got.Event.Source != source || got.Event.Rev != "abc" || string(got.Event.Context) != `{"pr":7}` {
		t.Fatalf("task spec %+v, event %+v", got.Spec, got.Event)
	}
	redelivered := h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("t2", "acme/app#7", "abc")))
	if redelivered.Task.ID != "t1" || len(redelivered.Events) != 0 || len(h.mem.Tasks()) != 1 {
		t.Fatalf("redelivery: task %s, %d events, %d tasks", redelivered.Task.ID, len(redelivered.Events), len(h.mem.Tasks()))
	}
	samePR := h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("t3", "acme/app#7", "def")))
	if samePR.Task.ID != "t3" || len(h.mem.Tasks()) != 2 {
		t.Errorf("a new rev should start a task, got %s and %d tasks", samePR.Task.ID, len(h.mem.Tasks()))
	}
}

func TestTwoSpecsOnOneEventStartTwoTasks(t *testing.T) {
	h := newHarness(t)
	h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("review-1", "acme/app#7", "abc")))
	second := h.evIn("nightly-1", "acme/app#7", "abc")
	second.Spec = nightly
	got := h.ok(h.svc.StartByEvent(h.ctx(), second))
	if got.Task.ID != "nightly-1" || len(h.mem.Tasks()) != 2 {
		t.Fatalf("the second spec got task %s and %d tasks exist", got.Task.ID, len(h.mem.Tasks()))
	}
	again := second
	again.ID = "nightly-2"
	if out := h.ok(h.svc.StartByEvent(h.ctx(), again)); out.Task.ID != "nightly-1" || len(out.Events) != 0 {
		t.Errorf("a redelivery to the second spec returned %s with %d events", out.Task.ID, len(out.Events))
	}
	for _, id := range []domain.TaskID{"review-1", "nightly-1"} {
		if got := h.get(id).State; got != domain.Waiting {
			t.Errorf("%s = %s, but a spec's task must not touch another spec's", id, got)
		}
	}
}

func TestPersonRunningASpecStartsATaskWithSpecAndEvent(t *testing.T) {
	h := newHarness(t)
	in := h.evInFrom("run-1", "manual", "acme/app#7", "abc")
	in.By = "ann"
	got := h.ok(h.svc.StartByEvent(h.ctx(), in)).Task
	if got.StartedBy != "ann" || got.Spec == nil || got.Event == nil || got.Event.Source != "manual" {
		t.Fatalf("task = %+v", got)
	}
	if got.Flow(cfg.Answerable) != domain.Evented {
		t.Errorf("a run by hand of a source nobody answers: flow = %s", got.Flow(cfg.Answerable))
	}
	h.slot("run-1")
	h.up("run-1")
	if _, err := h.svc.End(h.ctx(), "run-1"); !refused(err) {
		t.Errorf("a person ending a task with an event: err = %v", err)
	}
}

func TestSameKeyInTwoRepositoriesStartsTwoTasks(t *testing.T) {
	h := newHarness(t)
	h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("t1", "acme/app#7", "abc")))
	in := h.evIn("t2", "acme/app#7", "abc")
	in.Repo = blog
	h.ok(h.svc.StartByEvent(h.ctx(), in))
	for _, id := range []domain.TaskID{"t1", "t2"} {
		if got := h.get(id); got.State != domain.Waiting {
			t.Errorf("%s = %s", id, got.State)
		}
	}
}

func TestStartByEventIDTakenByOtherInputs(t *testing.T) {
	h := newHarness(t)
	h.ok(h.svc.StartByEvent(h.ctx(), h.evIn("t1", "acme/app#7", "abc")))
	_, err := h.svc.StartByEvent(h.ctx(), h.evIn("t1", "acme/app#8", "abc"))
	if !errors.Is(err, task.ErrIDTaken) {
		t.Errorf("err = %v", err)
	}
}

func TestStartByEventRefusesBadInput(t *testing.T) {
	h := newHarness(t)
	ok := h.evIn("t1", "acme/app#7", "abc")
	bad := map[string]func(*task.EventInput){
		"no id":        func(in *task.EventInput) { in.ID = "" },
		"no group":     func(in *task.EventInput) { in.Event.Group = "" },
		"no rev":       func(in *task.EventInput) { in.Event.Rev = "" },
		"no source":    func(in *task.EventInput) { in.Event.Source = "" },
		"no spec name": func(in *task.EventInput) { in.Spec.Name = "" },
		"no spec file": func(in *task.EventInput) { in.Spec.File = "" },
	}
	for name, mutate := range bad {
		in := ok
		mutate(&in)
		if _, err := h.svc.StartByEvent(h.ctx(), in); !errors.Is(err, task.ErrBadInput) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
}

func TestRetry(t *testing.T) {
	h := newHarness(t)
	old := h.reach(domain.Plain, domain.Failed)
	out := h.ok(h.svc.Retry(h.ctx(), "again", old))
	if out.Task.State != domain.Waiting || out.Task.Prompt != h.get(old).Prompt || out.Task.StartedBy != "ann" {
		t.Fatalf("task = %+v", out.Task)
	}
	want := []domain.EventKind{domain.KindState, domain.KindNote}
	if got := kinds(out.Events); len(got) != 2 || got[0] != want[0] || got[1] != want[1] || string(out.Events[1].Data) != fmt.Sprintf(`{"text":"retry of %s"}`, old) {
		t.Errorf("events = %v %s", got, out.Events[1].Data)
	}
	if again := h.ok(h.svc.Retry(h.ctx(), "again", old)); len(again.Events) != 0 || len(h.mem.Tasks()) != 2 {
		t.Errorf("a repeated retry made events or tasks: %d, %d", len(again.Events), len(h.mem.Tasks()))
	}
	if h.get(old).State != domain.Failed {
		t.Error("the old task changed")
	}
}

func TestRetryKeepsTheReviewPrompt(t *testing.T) {
	h := newHarness(t)
	h.ok(h.svc.StartByPerson(h.ctx(), task.StartInput{ID: "old", Repo: h.repo, Prompt: "p", ReviewPrompt: "check it", By: "ann"}))
	h.slot("old")
	h.ok(h.report("old", task.Report{Kind: task.PartFailed}))
	got := h.ok(h.svc.Retry(h.ctx(), "again", "old")).Task
	if got.ReviewPrompt != "check it" {
		t.Errorf("task = %+v", got)
	}
}

func TestRetryRefusals(t *testing.T) {
	h := newHarness(t)
	for _, state := range []domain.State{domain.Running, domain.Done, domain.Cancelled} {
		id := h.reach(domain.Plain, state)
		if _, err := h.svc.Retry(h.ctx(), h.id("new"), id); !refused(err) {
			t.Errorf("retry of %s: err = %v", state, err)
		}
	}
	ev := h.reach(domain.Answering, domain.Failed)
	if _, err := h.svc.Retry(h.ctx(), "new-ev", ev); !refused(err) {
		t.Errorf("retry of an event task: err = %v", err)
	}
	if _, err := h.svc.Retry(h.ctx(), "new", "missing"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("retry of nothing: err = %v", err)
	}
}
