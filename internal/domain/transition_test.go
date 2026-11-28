package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

var (
	allStates = []domain.State{
		domain.Waiting, domain.Starting, domain.Running, domain.Delivering,
		domain.Done, domain.Failed, domain.Superseded, domain.Cancelled,
	}
	allEvents = []domain.Event{
		domain.SlotFree, domain.PartsUp, domain.PartFailed, domain.BoxRecreated,
		domain.Published, domain.Delivered, domain.PersonEnded, domain.SessionEnded,
		domain.TimeLimit, domain.Steered, domain.NewerRev, domain.Cancel,
	}
	allFlows = []domain.Flow{domain.FreePrompt, domain.Triggered}
)

type edge struct {
	flow domain.Flow
	from domain.State
	on   domain.Event
}

func allowedEdges() map[edge]domain.State {
	free, trig := domain.FreePrompt, domain.Triggered
	allowed := map[edge]domain.State{
		{trig, domain.Running, domain.Published}:    domain.Delivering,
		{trig, domain.Delivering, domain.Delivered}: domain.Done,
		{free, domain.Running, domain.Published}:    domain.Done,
		{free, domain.Running, domain.PersonEnded}:  domain.Done,
	}
	for _, f := range allFlows {
		allowed[edge{f, domain.Waiting, domain.SlotFree}] = domain.Starting
		allowed[edge{f, domain.Starting, domain.PartsUp}] = domain.Running
		allowed[edge{f, domain.Starting, domain.PartFailed}] = domain.Failed
		allowed[edge{f, domain.Running, domain.BoxRecreated}] = domain.Running
		allowed[edge{f, domain.Done, domain.Steered}] = domain.Running
		allowed[edge{f, domain.Running, domain.SessionEnded}] = domain.Failed
		allowed[edge{f, domain.Running, domain.TimeLimit}] = domain.Failed
	}
	for _, s := range []domain.State{domain.Waiting, domain.Starting, domain.Running, domain.Delivering} {
		allowed[edge{trig, s, domain.NewerRev}] = domain.Superseded
		for _, f := range allFlows {
			allowed[edge{f, s, domain.Cancel}] = domain.Cancelled
		}
	}
	return allowed
}

func TestNextCoversEveryEdge(t *testing.T) {
	allowed := allowedEdges()
	refusedFrom := map[domain.State]int{}
	for _, e := range everyEdge() {
		if checkEdge(t, allowed, e) {
			refusedFrom[e.from]++
		}
	}
	for _, s := range allStates {
		if refusedFrom[s] == 0 {
			t.Errorf("no refused transition checked from %s", s)
		}
	}
}

func everyEdge() []edge {
	var edges []edge
	for _, f := range allFlows {
		for _, s := range allStates {
			for _, e := range allEvents {
				edges = append(edges, edge{f, s, e})
			}
		}
	}
	return edges
}

func checkEdge(t *testing.T, allowed map[edge]domain.State, e edge) (refused bool) {
	t.Helper()
	got, err := domain.Next(e.flow, e.from, e.on)
	want, ok := allowed[e]
	switch {
	case ok && (err != nil || got != want):
		t.Errorf("%s: %s on %s = %s, %v; want %s", e.flow, e.from, e.on, got, err, want)
	case !ok && (!errors.Is(err, domain.ErrRefused) || got != e.from):
		t.Errorf("%s: %s on %s = %s, %v; want refused", e.flow, e.from, e.on, got, err)
	}
	return !ok
}

func TestNextRefusesStatesOutsideTheEnum(t *testing.T) {
	if _, err := domain.Next(domain.FreePrompt, domain.State(0), domain.SlotFree); !errors.Is(err, domain.ErrRefused) {
		t.Fatalf("zero state: err = %v", err)
	}
	if _, err := domain.Next(domain.Flow(0), domain.Waiting, domain.SlotFree); !errors.Is(err, domain.ErrRefused) {
		t.Fatalf("zero flow: err = %v", err)
	}
}

func TestStateText(t *testing.T) {
	for _, s := range allStates {
		text, err := s.MarshalText()
		if err != nil {
			t.Fatalf("%d: %v", s, err)
		}
		var back domain.State
		if err := back.UnmarshalText(text); err != nil || back != s {
			t.Errorf("%q came back as %v, %v", text, back, err)
		}
	}
	if _, err := domain.ParseState("paused"); !errors.Is(err, domain.ErrUnknownState) {
		t.Errorf("paused: err = %v", err)
	}
	if _, err := domain.State(0).MarshalText(); !errors.Is(err, domain.ErrUnknownState) {
		t.Errorf("zero state marshalled: err = %v", err)
	}
}
