package task_test

import (
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// opCase is one operation with its expected result from each state, in the order of allStates, once
// for a person-started task with no reply target and once for a trigger-started one with a reply
// target. "x" is a state the flow cannot reach, and "-" a refusal. "=" is a repeat that changes nothing and "~" a change that keeps the state.
type opCase struct {
	name   string
	run    func(*harness, domain.TaskID) error
	person string
	trig   string
	pre    func(*harness)
}

func rep(kind task.ReportKind, gen int64) func(*harness, domain.TaskID) error {
	return func(h *harness, id domain.TaskID) error {
		return errOf(h.report(id, task.Report{Kind: kind, Gen: gen}))
	}
}

func viaService(call func(*task.Service, *harness, domain.TaskID) (task.Outcome, error)) func(*harness, domain.TaskID) error {
	return func(h *harness, id domain.TaskID) error {
		return errOf(call(h.svc, h, id))
	}
}

var opCases = []opCase{
	{
		name: "slot taken", run: rep(task.SlotTaken, 0),
		person: "starting = = x = = x =", trig: "starting = = = = = = =",
	},
	{
		name: "parts up", run: rep(task.PartsUp, 0),
		person: "- running = x = - x -", trig: "- running = = = - - -",
	},
	{
		name: "part failed", run: rep(task.PartFailed, 0),
		person: "- failed - x - = x -", trig: "- failed - - - = - -",
	},
	{
		name: "harness exited", run: rep(task.HarnessExited, 0),
		person: "- - failed x = = x =", trig: "- - failed = = = = =",
	},
	{
		name: "box recreated", run: rep(task.BoxRecreated, 1),
		person: "- - ~ x - - x -", trig: "- - ~ - - - - -",
	},
	{name: "publish", run: viaService(func(s *task.Service, h *harness, id domain.TaskID) (task.Outcome, error) {
		return wrapped(s.Publish(h.ctx(), id, task.PublishInput{Line: "ok", EntryURL: "https://pages.example/x", Digest: "dx"}))
	}), person: "- - done x - - x -", trig: "- - delivering - - - - -"},
	{name: "delivered", run: viaService(func(s *task.Service, h *harness, id domain.TaskID) (task.Outcome, error) {
		return wrapped(s.Delivered(h.ctx(), id, task.DeliveredInput{Target: "pr", ExternalID: "c1", Version: 1}))
	}), person: "- - - x - - x -", trig: "- - - done = - - -"},
	{
		name: "end", run: viaService(svcEnd),
		person: "- - done x = - x -", trig: "- - - - - - - -",
	},
	{
		name: "cancel", run: viaService(svcCancel),
		person: "cancelled cancelled cancelled x - - x =", trig: "cancelled cancelled cancelled cancelled - - - =",
	},
	{
		name: "time limit", run: viaService(svcTimeLimit), pre: func(h *harness) { h.advance(2 * cfg.TimeLimit) },
		person: "- - failed x - = x -", trig: "- - failed - - = - -",
	},
	{
		name: "steer", run: viaService(svcSteer),
		person: "- - = x running - x -", trig: "- - = - running - - -",
	},
	{
		name: "newer rev", run: newerRev,
		trig: "superseded superseded superseded superseded = = = =",
	},
}

func svcEnd(s *task.Service, h *harness, id domain.TaskID) (task.Outcome, error) {
	return wrapped(s.End(h.ctx(), id))
}

func svcCancel(s *task.Service, h *harness, id domain.TaskID) (task.Outcome, error) {
	return wrapped(s.Cancel(h.ctx(), id))
}

func svcTimeLimit(s *task.Service, h *harness, id domain.TaskID) (task.Outcome, error) {
	return wrapped(s.TimeLimit(h.ctx(), id))
}

func svcSteer(s *task.Service, h *harness, id domain.TaskID) (task.Outcome, error) {
	return wrapped(s.Steer(h.ctx(), id))
}

func newerRev(h *harness, id domain.TaskID) error {
	g := h.get(id).Trigger.Group
	return errOf(h.svc.StartByTrigger(h.ctx(), h.trigIn("newer-"+id, string(g), "rev-newer")))
}

func TestEveryOperationFromEveryState(t *testing.T) {
	for _, oc := range opCases {
		for _, flow := range []domain.Flow{personFlow, triggerFlow} {
			runCase(t, oc, flow)
		}
	}
}

func runCase(t *testing.T, oc opCase, flow domain.Flow) {
	want := oc.person
	if flow.Triggered {
		want = oc.trig
	}
	tokens := strings.Fields(want)
	for i, state := range allStates {
		if len(tokens) == 0 || tokens[i] == "x" {
			continue
		}
		t.Run(oc.name+"/"+flow.String()+"/"+state.String(), func(t *testing.T) {
			h := newHarness(t)
			id := h.reach(flow, state)
			checkToken(h, id, oc, tokens[i])
		})
	}
}

func snapshot(h *harness, id domain.TaskID) (domain.Task, int) {
	return h.get(id), len(h.mem.Events(id))
}

func checkToken(h *harness, id domain.TaskID, oc opCase, token string) {
	h.t.Helper()
	if oc.pre != nil {
		oc.pre(h)
	}
	before, nBefore := snapshot(h, id)
	err := oc.run(h, id)
	after, nAfter := snapshot(h, id)
	grew := nAfter > nBefore
	ok := false
	switch token {
	case "-":
		if !refused(err) || !reflect.DeepEqual(before, after) || grew {
			h.t.Fatalf("want a refusal that changes nothing: err %v, events %d to %d", err, nBefore, nAfter)
		}
		return
	case "=":
		ok = err == nil && reflect.DeepEqual(before, after) && !grew
	case "~":
		ok = err == nil && after.State == before.State && grew
	default:
		ok = err == nil && after.State.String() == token && grew
	}
	if !ok {
		h.t.Fatalf("want %q: err %v, state %s to %s, events %d to %d", token, err, before.State, after.State, nBefore, nAfter)
	}
	checkReplay(h, id, oc)
}

func checkReplay(h *harness, id domain.TaskID, oc opCase) {
	h.t.Helper()
	before, nBefore := snapshot(h, id)
	if err := oc.run(h, id); err != nil {
		h.t.Fatalf("replay: %v", err)
	}
	after, nAfter := snapshot(h, id)
	if !reflect.DeepEqual(before, after) || nBefore != nAfter {
		h.t.Fatalf("replay changed the task: events %d to %d", nBefore, nAfter)
	}
}
