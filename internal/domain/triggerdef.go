package domain

import (
	"errors"
	"fmt"
	"regexp"
	"strings"
)

type (
	TriggerName string
	EventName   string
)

var ErrBadTriggerDef = errors.New("bad trigger definition")

var triggerNamePattern = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]*$`)

// TriggerDef describes one trigger file. Prompt is a template, which is not rendered here.
// An empty ReviewPrompt means no review step. Reply asks for the result to be posted where the event came from.
type TriggerDef struct {
	Name         TriggerName
	Source       TriggerSource
	Events       []EventName
	Prompt       string
	Skills       []SkillName
	ReviewPrompt string
	Reply        bool
}

// Validate refuses a definition that could not start a task: it needs a file-safe name, a source,
// at least one event, a prompt, and no empty or repeated event or skill.
func (d TriggerDef) Validate() error {
	switch {
	case !triggerNamePattern.MatchString(string(d.Name)):
		return fmt.Errorf("%w: name %q is not lower-case letters, digits, - and _", ErrBadTriggerDef, d.Name)
	case d.Source == "":
		return fmt.Errorf("%w: %s has no source", ErrBadTriggerDef, d.Name)
	case len(d.Events) == 0:
		return fmt.Errorf("%w: %s names no events", ErrBadTriggerDef, d.Name)
	case strings.TrimSpace(d.Prompt) == "":
		return fmt.Errorf("%w: %s has no prompt", ErrBadTriggerDef, d.Name)
	}
	if err := distinct(d.Name, "event", d.Events); err != nil {
		return err
	}
	return distinct(d.Name, "skill", d.Skills)
}

// distinct wants every item non-empty and none twice.
func distinct[T ~string](name TriggerName, what string, items []T) error {
	seen := make(map[T]bool, len(items))
	for _, it := range items {
		if it == "" || seen[it] {
			return fmt.Errorf("%w: %s has an empty or repeated %s %q", ErrBadTriggerDef, name, what, it)
		}
		seen[it] = true
	}
	return nil
}
