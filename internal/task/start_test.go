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
		ID: "t1", Project: h.proj, Prompt: "look at it", By: "ann",
		Refs: map[domain.Dir]string{"app": "feature"},
	}
	out := h.ok(h.svc.StartByPerson(h.ctx(), in))
	got := out.Task
	if got.State != domain.Waiting || got.StartedBy != "ann" || got.Project != "shop" || got.Version != 1 || got.LastSeq != 1 || got.Refs["app"] != "feature" {
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

func TestStartByPersonKeepsReviewPromptAndReply(t *testing.T) {
	h := newHarness(t)
	in := task.StartInput{ID: "t1", Project: h.proj, Prompt: "p", ReviewPrompt: "check it", Reply: reply, By: "ann"}
	got := h.ok(h.svc.StartByPerson(h.ctx(), in)).Task
	if got.ReviewPrompt != "check it" || got.Reply != reply || got.Trigger != nil || got.Flow() != personReplies {
		t.Errorf("task = %+v", got)
	}
	if again := h.ok(h.svc.StartByPerson(h.ctx(), in)); len(again.Events) != 0 || len(h.mem.Tasks()) != 1 {
		t.Errorf("a repeated start made %d events, %d tasks", len(again.Events), len(h.mem.Tasks()))
	}
	for name, edit := range map[string]func(*task.StartInput){
		"other review prompt": func(in *task.StartInput) { in.ReviewPrompt = "other" },
		"other reply target":  func(in *task.StartInput) { in.Reply.Ref = "acme/app#8" },
		"no reply target":     func(in *task.StartInput) { in.Reply = domain.ReplyTarget{} },
	} {
		other := in
		edit(&other)
		if _, err := h.svc.StartByPerson(h.ctx(), other); !errors.Is(err, task.ErrIDTaken) {
			t.Errorf("same id, %s: err = %v", name, err)
		}
	}
}

func TestStartByPersonRefusesBadInput(t *testing.T) {
	h := newHarness(t)
	ok := task.StartInput{ID: "t1", Project: h.proj, Prompt: "p", By: "ann"}
	bad := map[string]func(*task.StartInput){
		"no id":               func(in *task.StartInput) { in.ID = "" },
		"no person":           func(in *task.StartInput) { in.By = "" },
		"no project":          func(in *task.StartInput) { in.Project = domain.Project{} },
		"no prompt":           func(in *task.StartInput) { in.Prompt = "" },
		"unknown dir":         func(in *task.StartInput) { in.Refs = map[domain.Dir]string{"nope": "main"} },
		"empty ref":           func(in *task.StartInput) { in.Refs = map[domain.Dir]string{"app": ""} },
		"half a reply target": func(in *task.StartInput) { in.Reply = domain.ReplyTarget{Source: "forge"} },
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

func TestStartByTriggerDedupesOnKey(t *testing.T) {
	h := newHarness(t)
	first := h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn("t1", "acme/app#7", "abc")))
	if first.Task.Flow() != triggerFlow || first.Task.Prompt != "show it" || first.Task.Trigger.Name != "pr-evidence" || first.Task.Reply != reply || first.Task.StartedBy != "trigger:forge" {
		t.Fatalf("task = %+v", first.Task)
	}
	redelivered := h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn("t2", "acme/app#7", "abc")))
	if redelivered.Task.ID != "t1" || len(redelivered.Events) != 0 || len(h.mem.Tasks()) != 1 {
		t.Fatalf("redelivery: task %s, %d events, %d tasks", redelivered.Task.ID, len(redelivered.Events), len(h.mem.Tasks()))
	}
	samePR := h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn("t3", "acme/app#7", "def")))
	if samePR.Task.ID != "t3" || len(h.mem.Tasks()) != 2 {
		t.Errorf("a new rev should start a task, got %s and %d tasks", samePR.Task.ID, len(h.mem.Tasks()))
	}
}

func TestSameKeyInTwoProjectsStartsTwoTasks(t *testing.T) {
	h := newHarness(t)
	other, err := domain.NewProject("blog", h.proj.Entries())
	if err != nil {
		t.Fatal(err)
	}
	h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn("t1", "acme/app#7", "abc")))
	in := h.trigIn("t2", "acme/app#7", "abc")
	in.Project = other
	h.ok(h.svc.StartByTrigger(h.ctx(), in))
	for _, id := range []domain.TaskID{"t1", "t2"} {
		if got := h.get(id); got.State != domain.Waiting {
			t.Errorf("%s = %s", id, got.State)
		}
	}
}

func TestStartByTriggerIDTakenByOtherInputs(t *testing.T) {
	h := newHarness(t)
	h.ok(h.svc.StartByTrigger(h.ctx(), h.trigIn("t1", "acme/app#7", "abc")))
	_, err := h.svc.StartByTrigger(h.ctx(), h.trigIn("t1", "acme/app#8", "abc"))
	if !errors.Is(err, task.ErrIDTaken) {
		t.Errorf("err = %v", err)
	}
}

func TestStartByTriggerRefusesBadInput(t *testing.T) {
	h := newHarness(t)
	ok := h.trigIn("t1", "acme/app#7", "abc")
	bad := map[string]func(*task.TriggerInput){
		"no id":               func(in *task.TriggerInput) { in.ID = "" },
		"no group":            func(in *task.TriggerInput) { in.Trigger.Group = "" },
		"no rev":              func(in *task.TriggerInput) { in.Trigger.Rev = "" },
		"no source":           func(in *task.TriggerInput) { in.Trigger.Source = "" },
		"no name":             func(in *task.TriggerInput) { in.Trigger.Name = "" },
		"half a reply target": func(in *task.TriggerInput) { in.Reply = domain.ReplyTarget{Ref: "acme/app#7"} },
	}
	for name, mutate := range bad {
		in := ok
		mutate(&in)
		if _, err := h.svc.StartByTrigger(h.ctx(), in); !errors.Is(err, task.ErrBadInput) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
}

func TestRetry(t *testing.T) {
	h := newHarness(t)
	old := h.reach(personFlow, domain.Failed)
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

func TestRetryKeepsReviewPromptAndReply(t *testing.T) {
	h := newHarness(t)
	h.ok(h.svc.StartByPerson(h.ctx(), task.StartInput{ID: "old", Project: h.proj, Prompt: "p", ReviewPrompt: "check it", Reply: reply, By: "ann"}))
	h.slot("old")
	h.ok(h.report("old", task.Report{Kind: task.PartFailed}))
	got := h.ok(h.svc.Retry(h.ctx(), "again", "old")).Task
	if got.ReviewPrompt != "check it" || got.Reply != reply {
		t.Errorf("task = %+v", got)
	}
}

func TestRetryRefusals(t *testing.T) {
	h := newHarness(t)
	for _, state := range []domain.State{domain.Running, domain.Done, domain.Cancelled} {
		id := h.reach(personFlow, state)
		if _, err := h.svc.Retry(h.ctx(), h.id("new"), id); !refused(err) {
			t.Errorf("retry of %s: err = %v", state, err)
		}
	}
	trig := h.reach(triggerFlow, domain.Failed)
	if _, err := h.svc.Retry(h.ctx(), "new-trig", trig); !refused(err) {
		t.Errorf("retry of a trigger task: err = %v", err)
	}
	if _, err := h.svc.Retry(h.ctx(), "new", "missing"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("retry of nothing: err = %v", err)
	}
}
