package domain

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"
)

// EventName is what happened, in the names a task spec's on lists.
type EventName string

const (
	EventPullRequest      EventName = "pull_request"
	EventSchedule         EventName = "schedule"
	EventWorkflowDispatch EventName = "workflow_dispatch"
)

// Incoming is an event that may start tasks. Action is the pull request type for a pull_request event
// and the cron that fired for a schedule one. Payload is the raw JSON object the prompts read.
// Source says where the event came from, and a spec does not choose by it.
type Incoming struct {
	Source  EventSource
	Name    EventName
	Action  string
	Repo    Repo
	Payload json.RawMessage
}

// Applies is true when the spec lists the event, and for a pull request or a schedule its type or
// cron too, and the spec's repositories, when it has any, include the event's repository.
func (s TaskSpec) Applies(in Incoming) bool {
	if len(s.Repositories) > 0 && !slices.Contains(s.Repositories, in.Repo) {
		return false
	}
	switch in.Name {
	case EventPullRequest:
		return s.On.PullRequest.hears(PullRequestType(in.Action))
	case EventSchedule:
		return slices.ContainsFunc(s.On.Schedule, func(sc ScheduleOn) bool {
			return normalizeCron(sc.Cron) == normalizeCron(in.Action)
		})
	case EventWorkflowDispatch:
		return s.On.WorkflowDispatch != nil
	}
	return false
}

func (p *PullRequestOn) hears(typ PullRequestType) bool {
	if p == nil {
		return false
	}
	if len(p.Types) == 0 {
		return slices.Contains(DefaultPullRequestTypes, typ)
	}
	return slices.Contains(p.Types, typ)
}

func normalizeCron(expr string) string { return strings.Join(strings.Fields(expr), " ") }

// ValidateSpecSet refuses two specs with the same name, which it compares without case.
func ValidateSpecSet(specs []TaskSpec) error {
	seen := make(map[string]SpecName, len(specs))
	for _, s := range specs {
		key := strings.ToLower(string(s.Name))
		if first, dup := seen[key]; dup {
			return fmt.Errorf("%w: two specs are named %q and %q", ErrBadTaskSpec, first, s.Name)
		}
		seen[key] = s.Name
	}
	return nil
}

// RepositoriesNotAllowed returns the repositories a spec names that the settings do not list.
func RepositoriesNotAllowed(spec TaskSpec, settings Settings) []Repo {
	var out []Repo
	for _, r := range spec.Repositories {
		if !slices.Contains(settings.Repositories, r) {
			out = append(out, r)
		}
	}
	return out
}
