package domain

import "fmt"

// StateGroup is one of the five marks the portal shows for a task: its states are grouped under them.
type StateGroup string

const (
	GroupQueued    StateGroup = "queued"
	GroupRunning   StateGroup = "running"
	GroupDone      StateGroup = "done"
	GroupFailed    StateGroup = "failed"
	GroupCancelled StateGroup = "cancelled"
)

// StateGroups lists the groups in the order the portal shows them.
var StateGroups = []StateGroup{GroupQueued, GroupRunning, GroupDone, GroupFailed, GroupCancelled}

var groupOf = map[State]StateGroup{
	Waiting: GroupQueued, Starting: GroupRunning, Running: GroupRunning, Delivering: GroupRunning,
	Done: GroupDone, Failed: GroupFailed, Cancelled: GroupCancelled, Superseded: GroupCancelled,
}

// Group is the mark a task in state s shows. A state with no group, which a stored task never has, is queued.
func (s State) Group() StateGroup {
	if g, ok := groupOf[s]; ok {
		return g
	}
	return GroupQueued
}

// States lists the task states under the group.
func (g StateGroup) States() []State {
	var out []State
	for s := Waiting; s <= Cancelled; s++ {
		if s.Group() == g {
			out = append(out, s)
		}
	}
	return out
}

func ParseStateGroup(text string) (StateGroup, error) {
	for _, g := range StateGroups {
		if string(g) == text {
			return g, nil
		}
	}
	return "", fmt.Errorf("%w group: %q", ErrUnknownState, text)
}

// EventStarterPrefix starts the StartedBy of a task that an event started with nobody running it by hand.
const EventStarterPrefix = "event:"

// TaskFilter narrows a list of tasks, and every field that is left empty narrows nothing. Spec and
// FromPrompt choose a left-list entry, and Starters reads only those two.
type TaskFilter struct {
	Spec SpecName
	// FromPrompt keeps the tasks a person started from a plain prompt, which have no spec.
	FromPrompt bool
	Repo       Repo
	StartedBy  string
	// ByEvent keeps the tasks an event started, with nobody running it by hand.
	ByEvent bool
	Group   StateGroup
	// Limit is the most tasks to list, newest first. Zero means 200.
	Limit int
}

// TaskListing is a task in a list. ReplacedBy names the task that took over a superseded one.
type TaskListing struct {
	Task       Task
	ReplacedBy TaskID
}

// TaskList is the newest tasks that match a filter. More is true when others matched but did not fit.
type TaskList struct {
	Items []TaskListing
	More  bool
}
