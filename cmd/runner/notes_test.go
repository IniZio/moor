package main

import (
	"testing"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

func noteIs(task, text string) func(wire.TaskWatchLine) bool {
	return func(l wire.TaskWatchLine) bool { return l.Task == task && l.Part == wire.NotePart && l.Text == text }
}

// notesOn opens a watch and returns the note lines among its first levels, which end with the session's.
func (g *rig) notesOn(t *testing.T, s *link.Session) []wire.TaskWatchLine {
	t.Helper()
	lines := watch(t, g.ctx, s)
	var notes []wire.TaskWatchLine
	for l := range lines {
		if l.Part == wire.NotePart {
			notes = append(notes, l)
		}
		if l.Part == wire.SessionPart {
			break
		}
	}
	return notes
}

func TestStartNotesTheStepsTheRunnerTook(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	lines := watch(t, g.ctx, s)
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{
		domain.NoteText(domain.PhaseDesk, string(g.portal.Agent().Runner)), domain.PhaseBox,
		domain.NoteText(domain.PhaseCheckout, "main at "+fakeCommit), domain.PhasePrompt,
	} {
		l := await(t, g.ctx, lines, "the note "+want, noteIs("t1", want))
		if l.Fence != (wire.Fence{Task: "t1", Run: 1, Gen: 1}) {
			t.Fatalf("note line %+v carries another launch", l)
		}
	}
	await(t, g.ctx, lines, "the session live", sessionIs("t1", "live"))
}

func TestANewWatchGetsTheNotesAgainAndARepeatedStartNotesNothingNew(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	for range 2 {
		if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
			t.Fatal(err)
		}
	}
	got := g.notesOn(t, s)
	if len(got) != 4 {
		t.Fatalf("a new watch got %d notes, want the 4 of the one launch: %+v", len(got), got)
	}
}
