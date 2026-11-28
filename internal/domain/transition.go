package domain

import (
	"errors"
	"fmt"
	"slices"
)

// Signal is something that happened to a task and may move its state.
type Signal uint8

const (
	SlotFree Signal = iota + 1
	PartsUp
	PartFailed
	BoxRecreated
	Published
	Delivered
	PersonEnded
	SessionEnded
	TimeLimit
	Steered
	NewerRev
	Cancel
	RunnerRevoked
	// ReviewSent says the review prompt went into the session. It moves nothing: the result is accepted on the next publish.
	ReviewSent
)

var signalNames = map[Signal]string{
	SlotFree:      "slot free",
	PartsUp:       "parts up",
	PartFailed:    "part failed",
	BoxRecreated:  "box recreated",
	Published:     "published",
	Delivered:     "delivered",
	PersonEnded:   "person ended",
	SessionEnded:  "session ended",
	TimeLimit:     "time limit",
	Steered:       "steered",
	NewerRev:      "newer rev",
	Cancel:        "cancel",
	RunnerRevoked: "machine revoked",
	ReviewSent:    "review sent",
}

func (s Signal) String() string {
	if name, ok := signalNames[s]; ok {
		return name
	}
	return fmt.Sprintf("Signal(%d)", uint8(s))
}

// Flow is what the state machine reads of a task. An event task can be superseded and cannot be ended
// by a person. One whose event can be answered is delivered before it is done. A plain task never is.
type Flow uint8

const (
	Plain Flow = iota
	Evented
	Answering
)

func (f Flow) String() string {
	switch f {
	case Plain:
		return "plain"
	case Evented:
		return "event"
	case Answering:
		return "event answering"
	}
	return fmt.Sprintf("Flow(%d)", uint8(f))
}

var ErrRefused = errors.New("transition refused")

type rule struct {
	from State
	on   Signal
	to   State
	when func(Flow) bool
}

func always(Flow) bool        { return true }
func byEvent(f Flow) bool     { return f != Plain }
func byPerson(f Flow) bool    { return f == Plain }
func answering(f Flow) bool   { return f == Answering }
func unanswering(f Flow) bool { return f != Answering }

var rules = []rule{
	{Waiting, SlotFree, Starting, always},
	{Starting, PartsUp, Running, always},
	{Starting, PartFailed, Failed, always},
	{Running, BoxRecreated, Running, always},
	{Running, ReviewSent, Running, always},
	{Running, Published, Delivering, answering},
	{Delivering, Delivered, Done, answering},
	{Running, Published, Done, unanswering},
	{Running, PersonEnded, Done, byPerson},
	{Done, Steered, Running, always},
	{Running, SessionEnded, Failed, always},
	{Running, TimeLimit, Failed, always},
	{Starting, RunnerRevoked, Failed, always},
	{Running, RunnerRevoked, Failed, always},
	{Waiting, NewerRev, Superseded, byEvent},
	{Starting, NewerRev, Superseded, byEvent},
	{Running, NewerRev, Superseded, byEvent},
	{Delivering, NewerRev, Superseded, byEvent},
	{Waiting, Cancel, Cancelled, always},
	{Starting, Cancel, Cancelled, always},
	{Running, Cancel, Cancelled, always},
	{Delivering, Cancel, Cancelled, always},
}

// Handles tells whether any rule gives the flow this event, whatever the state.
func Handles(flow Flow, on Signal) bool {
	return slices.ContainsFunc(rules, func(r rule) bool { return r.on == on && r.when(flow) })
}

// Next gives the state a task in this flow moves to on an event. A refused event leaves the state as it was.
func Next(flow Flow, from State, on Signal) (State, error) {
	for _, r := range rules {
		if r.from == from && r.on == on && r.when(flow) {
			return r.to, nil
		}
	}
	return from, fmt.Errorf("%w: %s on %s in a %s task", ErrRefused, on, from, flow)
}
