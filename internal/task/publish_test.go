package task_test

import (
	"encoding/json"
	"errors"
	"slices"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func fxKinds(fx []task.Effect) []task.EffectKind {
	out := make([]task.EffectKind, len(fx))
	for i, e := range fx {
		out[i] = e.Kind
	}
	return out
}

func wantEffect(t *testing.T, out task.Outcome, kind task.EffectKind, version int) {
	t.Helper()
	if len(out.Effects) != 1 || out.Effects[0].Kind != kind || out.Effects[0].Version != version {
		t.Fatalf("effects = %+v, want kind %d for version %d", out.Effects, kind, version)
	}
}

func TestPublishOfATaskThatAnswersNothingEnds(t *testing.T) {
	for _, flow := range []domain.Flow{domain.Plain, domain.Evented} {
		t.Run(flow.String(), func(t *testing.T) {
			h := newHarness(t)
			id := h.reach(flow, domain.Running)
			out := h.publish(id, "d1")
			got := h.get(id)
			if got.State != domain.Done || len(out.Effects) != 0 || got.KeepUntil != h.clock.Now().Add(cfg.KeepWindow) || got.EndedAt.IsZero() {
				t.Fatalf("task = %+v, effects %v", got, out.Effects)
			}
			if k := kinds(out.Events); len(k) != 2 || k[0] != domain.KindPublished || k[1] != domain.KindState {
				t.Errorf("events = %v", k)
			}
		})
	}
}

func TestPublishOfAnAnsweredEventDeliversThenDelivered(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	out := h.publish(id, "d1")
	wantEffect(t, out, task.Deliver, 1)
	if h.get(id).State != domain.Delivering {
		t.Fatalf("state = %s", h.get(id).State)
	}
	done := h.ok(h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Target: "pr 7", ExternalID: "c1", Version: 1}))
	if h.get(id).State != domain.Done || kinds(done.Events)[0] != domain.KindDelivered {
		t.Fatalf("state %s, events %v", h.get(id).State, kinds(done.Events))
	}
	var data domain.DeliveredData
	if err := json.Unmarshal(done.Events[0].Data, &data); err != nil || data.ExternalID != "c1" || data.Version != 1 {
		t.Errorf("delivered data = %+v, %v", data, err)
	}
}

func TestTheConfigDecidesWhichSourcesAreAnswered(t *testing.T) {
	h := newHarness(t)
	silent := task.New(h.mem, h.clock, task.Config{})
	id := h.id("ev")
	h.ok(silent.StartByEvent(h.ctx(), h.evIn(id, "acme/app#7", "abc")))
	h.ok(silent.Report(h.ctx(), id, task.Report{Kind: task.SlotTaken, Runner: "r1"}))
	h.ok(silent.Report(h.ctx(), id, task.Report{Kind: task.PartsUp, Run: 1}))
	out := h.ok(silent.Publish(h.ctx(), id, task.PublishInput{Line: "l", EntryURL: "u", Digest: "d1"}))
	if h.get(id).State != domain.Done || len(out.Effects) != 0 {
		t.Errorf("an event of a source nobody answers: state %s, effects %v", h.get(id).State, out.Effects)
	}
}

func TestSameDigestTwiceIsOneVersion(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	h.publish(id, "d1")
	before := len(h.mem.Events(id))
	again := h.publish(id, "d1")
	versions, _ := h.mem.Versions(h.ctx(), id)
	if len(versions) != 1 || len(again.Events) != 0 || len(h.mem.Events(id)) != before {
		t.Fatalf("versions %d, events %d", len(versions), len(h.mem.Events(id))-before)
	}
	wantEffect(t, again, task.Deliver, 1)
}

func TestPublishRefusals(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	for name, in := range map[string]task.PublishInput{
		"no digest": {Line: "l", EntryURL: "u"},
		"no url":    {Line: "l", Digest: "d"},
	} {
		if _, err := h.svc.Publish(h.ctx(), id, in); !errors.Is(err, task.ErrBadInput) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	h.publish(id, "d1")
	if _, err := h.svc.Publish(h.ctx(), id, task.PublishInput{Line: "l", EntryURL: "u", Digest: "d2"}); !refused(err) {
		t.Errorf("a new publish while delivering: err = %v", err)
	}
	if v, _ := h.mem.Versions(h.ctx(), id); len(v) != 1 {
		t.Errorf("versions = %d", len(v))
	}
}

func TestReviewPromptPath(t *testing.T) {
	h := newHarness(t)
	id := h.start(domain.Answering, "check it")
	h.running(id)

	first := h.publish(id, "d1")
	wantEffect(t, first, task.SendReviewTurn, 1)
	if got := h.get(id); got.State != domain.Running || kinds(first.Events)[0] != domain.KindPublished || len(first.Events) != 1 {
		t.Fatalf("after the first publish: state %s, events %v", got.State, kinds(first.Events))
	}

	replay := h.publish(id, "d1")
	wantEffect(t, replay, task.SendReviewTurn, 1)
	if len(replay.Events) != 0 {
		t.Error("a repeated first publish wrote events")
	}

	second := h.publish(id, "d2")
	wantEffect(t, second, task.Deliver, 2)
	if h.get(id).State != domain.Delivering {
		t.Fatalf("state = %s", h.get(id).State)
	}
	if late := h.publish(id, "d1"); len(late.Effects) != 0 || len(late.Events) != 0 {
		t.Errorf("a late repeat of the first version: %+v", late)
	}

	h.ok(h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Target: "pr 7", ExternalID: "c1", Version: 2}))
	h.ok(h.svc.Steer(h.ctx(), id))
	third := h.publish(id, "d3")
	wantEffect(t, third, task.Deliver, 3)
}

func TestReviewPromptOnAnyKindOfTask(t *testing.T) {
	cases := []struct {
		flow     domain.Flow
		accepted domain.State
	}{
		{domain.Plain, domain.Done},
		{domain.Evented, domain.Done},
		{domain.Answering, domain.Delivering},
	}
	for _, c := range cases {
		t.Run(c.flow.String(), func(t *testing.T) {
			h := newHarness(t)
			id := h.start(c.flow, "check it")
			h.running(id)
			wantEffect(t, h.publish(id, "d1"), task.SendReviewTurn, 1)
			if got := h.get(id).State; got != domain.Running {
				t.Fatalf("after the first publish: state %s", got)
			}
			second := h.publish(id, "d2")
			if got := h.get(id).State; got != c.accepted || len(second.Effects) != btoi(c.accepted == domain.Delivering) {
				t.Fatalf("after the second publish: state %s, effects %v", got, second.Effects)
			}
		})
	}
}

func btoi(b bool) int {
	if b {
		return 1
	}
	return 0
}

func TestNoReviewStepWithoutAReviewPrompt(t *testing.T) {
	for _, flow := range []domain.Flow{domain.Plain, domain.Evented, domain.Answering} {
		t.Run(flow.String(), func(t *testing.T) {
			h := newHarness(t)
			id := h.reach(flow, domain.Running)
			out := h.publish(id, "d1")
			if slices.Contains(fxKinds(out.Effects), task.SendReviewTurn) || h.get(id).State == domain.Running {
				t.Fatalf("state %s, effects %v", h.get(id).State, fxKinds(out.Effects))
			}
		})
	}
}

func TestDeliveredIsRefusedForATaskThatAnswersNothing(t *testing.T) {
	for _, flow := range []domain.Flow{domain.Plain, domain.Evented} {
		h := newHarness(t)
		id := h.reach(flow, domain.Running)
		h.publish(id, "d1")
		if _, err := h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Version: 1}); !refused(err) {
			t.Errorf("%s: err = %v", flow, err)
		}
	}
}

func TestDeliveredRefusals(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	h.publish(id, "d1")
	if _, err := h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Version: 2}); !refused(err) {
		t.Errorf("a delivery for a version that does not exist: err = %v", err)
	}
	if h.get(id).State != domain.Delivering {
		t.Error("a refused delivery moved the task")
	}
}

func TestDeliveringIsRetriedByPublishReplay(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Delivering)
	wantEffect(t, h.publish(id, "d1"), task.Deliver, 1)
	h.ok(h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Version: 1}))
	if out := h.publish(id, "d1"); len(out.Effects) != 0 {
		t.Errorf("a done task owes nothing: %+v", out.Effects)
	}
}

func TestPublishKeepsThePageWithItsVersionAndOnlyThere(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	page := []byte("<h1>It works</h1>")
	h.ok(h.svc.Publish(h.ctx(), id, task.PublishInput{Line: "it works", EntryURL: "https://pages.example/1", Digest: "d1", Page: page}))
	got, err := h.mem.Page(h.ctx(), id, 1)
	if err != nil || string(got) != string(page) {
		t.Fatalf("page = %q, %v", got, err)
	}
	if _, err := h.mem.Page(h.ctx(), id, 2); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("a version that does not exist: err = %v", err)
	}
}

func TestTheEntryURLNamesItsVersionWhereItAsksForIt(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	h.ok(h.svc.Publish(h.ctx(), id, task.PublishInput{Line: "l", EntryURL: "https://portal.example/results/x/{version}", Digest: "d1"}))
	v, _ := h.mem.Versions(h.ctx(), id)
	if len(v) != 1 || v[0].EntryURL != "https://portal.example/results/x/1" {
		t.Fatalf("versions = %+v", v)
	}
	h.ok(h.svc.Steer(h.ctx(), id))
	h.ok(h.svc.Publish(h.ctx(), id, task.PublishInput{Line: "l", EntryURL: "https://portal.example/results/x/{version}", Digest: "d2"}))
	if v, _ := h.mem.Versions(h.ctx(), id); v[1].EntryURL != "https://portal.example/results/x/2" {
		t.Fatalf("second version = %+v", v[1])
	}
}
