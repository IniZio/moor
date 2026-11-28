package task_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func TestARunnerNoteIsRecordedOnceWhateverHowOftenItIsSent(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Starting)
	text := domain.NoteText(domain.PhaseDesk, "m-1")
	first := h.ok(h.report(id, task.Report{Kind: task.Noted, Text: text}))
	if k := kinds(first.Events); len(k) != 1 || k[0] != domain.KindNote {
		t.Fatalf("first note wrote %v", k)
	}
	if again := h.ok(h.report(id, task.Report{Kind: task.Noted, Text: text})); len(again.Events) != 0 {
		t.Fatalf("the same note again wrote %v", kinds(again.Events))
	}
	other := h.ok(h.report(id, task.Report{Kind: task.Noted, Text: domain.NoteText(domain.PhaseBox, "box:dev")}))
	if len(other.Events) != 1 {
		t.Fatalf("another note wrote %v", kinds(other.Events))
	}
}

func TestANoteFromAnOlderRunIsStale(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Starting)
	_, err := h.svc.Report(h.ctx(), id, task.Report{Kind: task.Noted, Run: h.get(id).Run + 1, Text: "desk up"})
	if !errors.Is(err, task.ErrStaleReport) {
		t.Fatalf("err = %v", err)
	}
}

func TestNotesNeedATaskThatLives(t *testing.T) {
	h := newHarness(t)
	id := h.reach(domain.Plain, domain.Cancelled)
	if _, err := h.report(id, task.Report{Kind: task.Noted, Text: "desk up"}); !refused(err) {
		t.Fatalf("err = %v", err)
	}
	if _, err := h.report(h.reach(domain.Plain, domain.Starting), task.Report{Kind: task.Noted}); !errors.Is(err, task.ErrBadInput) {
		t.Fatalf("an empty note: err = %v", err)
	}
}
