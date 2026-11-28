// Package domain holds tasks, projects, trigger definitions and the task state machine. It does no input or output.
package domain

import (
	"errors"
	"fmt"
)

// State is where a task is in its life. The zero value is no state, so a stored state must be parsed.
type State uint8

const (
	Waiting State = iota + 1
	Starting
	Running
	Delivering
	Done
	Failed
	Superseded
	Cancelled
)

// Live is true while a task still holds, or waits for, a slot.
func (s State) Live() bool {
	return s == Waiting || s == Starting || s == Running || s == Delivering
}

var ErrUnknownState = errors.New("unknown task state")

var stateNames = map[State]string{
	Waiting:    "waiting",
	Starting:   "starting",
	Running:    "running",
	Delivering: "delivering",
	Done:       "done",
	Failed:     "failed",
	Superseded: "superseded",
	Cancelled:  "cancelled",
}

func (s State) String() string {
	if name, ok := stateNames[s]; ok {
		return name
	}
	return fmt.Sprintf("State(%d)", uint8(s))
}

func ParseState(text string) (State, error) {
	for s, name := range stateNames {
		if name == text {
			return s, nil
		}
	}
	return 0, fmt.Errorf("%w: %q", ErrUnknownState, text)
}

func (s State) MarshalText() ([]byte, error) {
	name, ok := stateNames[s]
	if !ok {
		return nil, fmt.Errorf("%w: %d", ErrUnknownState, uint8(s))
	}
	return []byte(name), nil
}

func (s *State) UnmarshalText(text []byte) error {
	parsed, err := ParseState(string(text))
	if err != nil {
		return err
	}
	*s = parsed
	return nil
}
