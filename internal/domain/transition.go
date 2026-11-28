package domain

import (
	"errors"
	"fmt"
	"slices"
)

// Event is something that happened to a task and may move its state.
type Event uint8

const (
	SlotFree Event = iota + 1
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
)

var eventNames = map[Event]string{
	SlotFree:     "slot free",
	PartsUp:      "parts up",
	PartFailed:   "part failed",
	BoxRecreated: "box recreated",
	Published:    "published",
	Delivered:    "delivered",
	PersonEnded:  "person ended",
	SessionEnded: "session ended",
	TimeLimit:    "time limit",
	Steered:      "steered",
	NewerRev:     "newer rev",
	Cancel:       "cancel",
}

func (e Event) String() string {
	if name, ok := eventNames[e]; ok {
		return name
	}
	return fmt.Sprintf("Event(%d)", uint8(e))
}

// Flow tells a free-prompt task from one a trigger started. Only the latter delivers and is superseded.
type Flow uint8

const (
	FreePrompt Flow = iota + 1
	Triggered
)

func (f Flow) String() string {
	switch f {
	case FreePrompt:
		return "free prompt"
	case Triggered:
		return "triggered"
	}
	return fmt.Sprintf("Flow(%d)", uint8(f))
}

var ErrRefused = errors.New("transition refused")

type rule struct {
	from  State
	on    Event
	to    State
	flows []Flow
}

var (
	anyFlow   = []Flow{FreePrompt, Triggered}
	triggered = []Flow{Triggered}
	free      = []Flow{FreePrompt}
)

var rules = []rule{
	{Waiting, SlotFree, Starting, anyFlow},
	{Starting, PartsUp, Running, anyFlow},
	{Starting, PartFailed, Failed, anyFlow},
	{Running, BoxRecreated, Running, anyFlow},
	{Running, Published, Delivering, triggered},
	{Delivering, Delivered, Done, triggered},
	{Running, Published, Done, free},
	{Running, PersonEnded, Done, free},
	{Done, Steered, Running, anyFlow},
	{Running, SessionEnded, Failed, anyFlow},
	{Running, TimeLimit, Failed, anyFlow},
	{Waiting, NewerRev, Superseded, triggered},
	{Starting, NewerRev, Superseded, triggered},
	{Running, NewerRev, Superseded, triggered},
	{Delivering, NewerRev, Superseded, triggered},
	{Waiting, Cancel, Cancelled, anyFlow},
	{Starting, Cancel, Cancelled, anyFlow},
	{Running, Cancel, Cancelled, anyFlow},
	{Delivering, Cancel, Cancelled, anyFlow},
}

// Next gives the state a task in this flow moves to on an event. A refused event leaves the state as it was.
func Next(flow Flow, from State, on Event) (State, error) {
	for _, r := range rules {
		if r.from == from && r.on == on && slices.Contains(r.flows, flow) {
			return r.to, nil
		}
	}
	return from, fmt.Errorf("%w: %s on %s in a %s task", ErrRefused, on, from, flow)
}
