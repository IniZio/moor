package task_test

import (
	"sync"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
)

func reviewNotes(rows []domain.EventRow) int {
	n := 0
	for _, r := range rows {
		if r.Kind == domain.KindNote && string(r.Data) == `{"text":"review sent"}` {
			n++
		}
	}
	return n
}

func TestTheReviewTurnIsOwedOnlyAfterTheFirstPublishOfATaskWithAReviewPrompt(t *testing.T) {
	h := newHarness(t)
	id := h.start(domain.Answering, "check it")
	h.running(id)
	if out := h.ok(h.svc.ReviewTurn(h.ctx(), id)); len(out.Effects) != 0 || len(out.Events) != 0 {
		t.Fatalf("before any publish: %+v", out)
	}
	h.publish(id, "d1")
	out := h.ok(h.svc.ReviewTurn(h.ctx(), id))
	wantEffect(t, out, task.SendReviewTurn, 1)
	if k := kinds(out.Events); len(k) != 1 || k[0] != domain.KindNote || h.get(id).State != domain.Running {
		t.Fatalf("claim wrote %v, state %s", k, h.get(id).State)
	}
	if again := h.ok(h.svc.ReviewTurn(h.ctx(), id)); len(again.Effects) != 0 || len(again.Events) != 0 {
		t.Fatalf("a second claim: %+v", again)
	}
	h.publish(id, "d2")
	if late := h.ok(h.svc.ReviewTurn(h.ctx(), id)); len(late.Effects) != 0 || len(late.Events) != 0 {
		t.Fatalf("after the second publish: %+v", late)
	}
}

func TestNoReviewTurnWithoutAReviewPrompt(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Running)
	h.publish(id, "d1")
	if out := h.ok(h.svc.ReviewTurn(h.ctx(), id)); len(out.Effects) != 0 || len(out.Events) != 0 {
		t.Fatalf("a task with no review prompt: %+v", out)
	}
}

func TestTwoReplicasClaimTheReviewTurnOnce(t *testing.T) {
	mem := tasktest.NewMemory()
	h := newHarnessOn(t, mem, nil)
	id := h.start(domain.Answering, "check it")
	h.running(id)
	h.publish(id, "d1")
	replicas := []*task.Service{task.New(mem, h.clock, cfg), task.New(mem, h.clock, cfg), task.New(mem, h.clock, cfg)}
	var wg sync.WaitGroup
	claimed := make(chan task.Outcome, 24)
	for range 8 {
		for _, svc := range replicas {
			wg.Go(func() {
				out, err := svc.ReviewTurn(h.ctx(), id)
				if err != nil {
					t.Error(err)
				}
				claimed <- out
			})
		}
	}
	wg.Wait()
	close(claimed)
	won := 0
	for out := range claimed {
		won += len(out.Effects)
	}
	if won != 1 || reviewNotes(mem.Events(id)) != 1 {
		t.Fatalf("%d claims won, %d review notes", won, reviewNotes(mem.Events(id)))
	}
}

func TestARestartedPortalDoesNotSendTheReviewAgain(t *testing.T) {
	mem := tasktest.NewMemory()
	h := newHarnessOn(t, mem, nil)
	id := h.start(domain.Plain, "check it")
	h.running(id)
	h.publish(id, "d1")
	wantEffect(t, h.ok(h.svc.ReviewTurn(h.ctx(), id)), task.SendReviewTurn, 1)
	restarted := task.New(mem, h.clock, cfg)
	if out := h.ok(restarted.ReviewTurn(h.ctx(), id)); len(out.Effects) != 0 {
		t.Fatalf("after a restart: %+v", out)
	}
}

func TestASessionThatEndsHoldingAFirstPublishAcceptsIt(t *testing.T) {
	cases := []struct {
		flow  domain.Flow
		state domain.State
		fx    int
	}{
		{domain.Plain, domain.Done, 0},
		{domain.Evented, domain.Done, 0},
		{domain.Answering, domain.Delivering, 1},
	}
	for _, c := range cases {
		t.Run(c.flow.String(), func(t *testing.T) {
			h := newHarness(t)
			id := h.start(c.flow, "check it")
			h.running(id)
			h.publish(id, "d1")
			out := h.ok(h.report(id, task.Report{Kind: task.HarnessExited}))
			if got := h.get(id).State; got != c.state || len(out.Effects) != c.fx {
				t.Fatalf("state %s, effects %+v", got, out.Effects)
			}
			again := h.ok(h.report(id, task.Report{Kind: task.HarnessExited}))
			if len(again.Events) != 0 {
				t.Errorf("a repeated exit wrote %v", kinds(again.Events))
			}
		})
	}
}

func TestTheVersionAHeldSessionAcceptsIsTheOneDelivered(t *testing.T) {
	h := newHarness(t)
	id := h.start(domain.Answering, "check it")
	h.running(id)
	h.publish(id, "d1")
	wantEffect(t, h.ok(h.report(id, task.Report{Kind: task.HarnessExited})), task.Deliver, 1)
}

func TestASessionThatEndsWithNothingPublishedStillFails(t *testing.T) {
	h := newHarness(t)
	id := h.start(domain.Answering, "check it")
	h.running(id)
	h.ok(h.report(id, task.Report{Kind: task.HarnessExited}))
	if got := h.get(id).State; got != domain.Failed {
		t.Fatalf("state = %s", got)
	}
}
