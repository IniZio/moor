package task_test

import (
	"encoding/json"
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func TestAFailedAnswerIsRecordedWithItsReasonAndTheTaskStaysDelivering(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	h.publish(id, "d1")
	out := h.ok(h.svc.AnswerFailed(h.ctx(), id, 1, "comment on pull request #7: 502 Bad Gateway"))
	if got := h.get(id); got.State != domain.Delivering || got.Failure != "" {
		t.Fatalf("task = %+v", got)
	}
	if len(out.Events) != 1 || out.Events[0].Kind != domain.KindError {
		t.Fatalf("events = %v", kinds(out.Events))
	}
	var data domain.ErrorData
	if err := json.Unmarshal(out.Events[0].Data, &data); err != nil || data.Text != "the answer for version 1 was not posted: comment on pull request #7: 502 Bad Gateway" {
		t.Errorf("error data = %+v, %v", data, err)
	}
}

func TestTheDeliveryAFailedAnswerOwesIsReportedAgainByTheSamePublish(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	h.publish(id, "d1")
	h.ok(h.svc.AnswerFailed(h.ctx(), id, 1, "no route"))
	wantEffect(t, h.publish(id, "d1"), task.Deliver, 1)
}

func TestAnAnswerFailsOnlyWhileTheTaskIsDelivering(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Answering, domain.Running)
	if _, err := h.svc.AnswerFailed(h.ctx(), id, 1, "x"); !errors.Is(err, domain.ErrRefused) {
		t.Errorf("a running task: %v", err)
	}
	h.publish(id, "d1")
	h.ok(h.svc.Delivered(h.ctx(), id, task.DeliveredInput{Target: "pr 7", ExternalID: "c1", Version: 1}))
	if _, err := h.svc.AnswerFailed(h.ctx(), id, 1, "x"); !errors.Is(err, domain.ErrRefused) {
		t.Errorf("a done task: %v", err)
	}
}
