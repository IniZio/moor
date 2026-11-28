package domain

import (
	"errors"
	"fmt"
	"regexp"
	"strings"
)

type (
	SpecName        string
	PullRequestType string
)

// The pull request activity types a task spec may name.
const (
	PROpened         PullRequestType = "opened"
	PRSynchronize    PullRequestType = "synchronize"
	PRReopened       PullRequestType = "reopened"
	PRReadyForReview PullRequestType = "ready_for_review"
	PRClosed         PullRequestType = "closed"
)

var knownPullRequestTypes = map[PullRequestType]bool{
	PROpened: true, PRSynchronize: true, PRReopened: true, PRReadyForReview: true, PRClosed: true,
}

const cronFields = 5

var ErrBadTaskSpec = errors.New("bad task spec")

var specNamePattern = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]*$`)

// PullRequestOn starts a task on pull request activity. No types means the forge's default ones.
type PullRequestOn struct {
	Types []PullRequestType
}

// ScheduleOn starts a task on a cron schedule of five fields.
type ScheduleOn struct {
	Cron string
}

// On lists what starts tasks of a spec. WorkflowDispatch lets a person start one with Run.
type On struct {
	PullRequest      *PullRequestOn
	Schedule         *ScheduleOn
	WorkflowDispatch bool
}

// Concurrency says what a newer event does to a task of the same spec and event group. With
// CancelInProgress it supersedes the older live tasks, and without it both go on.
type Concurrency struct {
	CancelInProgress bool
}

// TaskSpec is what a task spec file says. Prompt is a template, which is not rendered here, and an
// empty ReviewPrompt means no review step.
type TaskSpec struct {
	Name         SpecName
	On           On
	Concurrency  Concurrency
	Prompt       string
	ReviewPrompt string
}

// Validate refuses a spec that could not start a task: it needs a file-safe name, at least one
// way to start, a prompt, known and distinct pull request types, and a five-field cron.
func (s TaskSpec) Validate() error {
	switch {
	case !specNamePattern.MatchString(string(s.Name)):
		return fmt.Errorf("%w: name %q is not lower-case letters, digits, - and _", ErrBadTaskSpec, s.Name)
	case s.On.PullRequest == nil && s.On.Schedule == nil && !s.On.WorkflowDispatch:
		return fmt.Errorf("%w: %s has nothing in on", ErrBadTaskSpec, s.Name)
	case strings.TrimSpace(s.Prompt) == "":
		return fmt.Errorf("%w: %s has no prompt", ErrBadTaskSpec, s.Name)
	}
	if err := s.On.PullRequest.validate(s.Name); err != nil {
		return err
	}
	return s.On.Schedule.validate(s.Name)
}

func (p *PullRequestOn) validate(name SpecName) error {
	if p == nil {
		return nil
	}
	seen := make(map[PullRequestType]bool, len(p.Types))
	for _, typ := range p.Types {
		if !knownPullRequestTypes[typ] || seen[typ] {
			return fmt.Errorf("%w: %s has an unknown or repeated pull_request type %q", ErrBadTaskSpec, name, typ)
		}
		seen[typ] = true
	}
	return nil
}

func (s *ScheduleOn) validate(name SpecName) error {
	if s != nil && len(strings.Fields(s.Cron)) != cronFields {
		return fmt.Errorf("%w: %s has cron %q, which is not %d fields", ErrBadTaskSpec, name, s.Cron, cronFields)
	}
	return nil
}
