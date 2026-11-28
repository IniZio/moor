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

// Flow is what the state machine reads of a task. A task a trigger started can be superseded and
// cannot be ended by a person. A task with a reply target is delivered before it is done.
type Flow struct {
	Triggered bool
	Replies   bool
}

func (f Flow) String() string {
	who := "person-started"
	if f.Triggered {
		who = "trigger-started"
	}
	if f.Replies {
		return who + " replying"
	}
	return who
}

var ErrRefused = errors.New("transition refused")

type rule struct {
	from State
	on   Event
	to   State
	when func(Flow) bool
}

func always(Flow) bool         { return true }
func byTrigger(f Flow) bool    { return f.Triggered }
func byPerson(f Flow) bool     { return !f.Triggered }
func withReply(f Flow) bool    { return f.Replies }
func withoutReply(f Flow) bool { return !f.Replies }

var rules = []rule{
	{Waiting, SlotFree, Starting, always},
	{Starting, PartsUp, Running, always},
	{Starting, PartFailed, Failed, always},
	{Running, BoxRecreated, Running, always},
	{Running, Published, Delivering, withReply},
	{Delivering, Delivered, Done, withReply},
	{Running, Published, Done, withoutReply},
	{Running, PersonEnded, Done, byPerson},
	{Done, Steered, Running, always},
	{Running, SessionEnded, Failed, always},
	{Running, TimeLimit, Failed, always},
	{Waiting, NewerRev, Superseded, byTrigger},
	{Starting, NewerRev, Superseded, byTrigger},
	{Running, NewerRev, Superseded, byTrigger},
	{Delivering, NewerRev, Superseded, byTrigger},
	{Waiting, Cancel, Cancelled, always},
	{Starting, Cancel, Cancelled, always},
	{Running, Cancel, Cancelled, always},
	{Delivering, Cancel, Cancelled, always},
}

// Handles tells whether any rule gives the flow this event, whatever the state.
func Handles(flow Flow, on Event) bool {
	return slices.ContainsFunc(rules, func(r rule) bool { return r.on == on && r.when(flow) })
}

// Next gives the state a task in this flow moves to on an event. A refused event leaves the state as it was.
func Next(flow Flow, from State, on Event) (State, error) {
	for _, r := range rules {
		if r.from == from && r.on == on && r.when(flow) {
			return r.to, nil
		}
	}
	return from, fmt.Errorf("%w: %s on %s in a %s task", ErrRefused, on, from, flow)
}
