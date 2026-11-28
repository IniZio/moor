package domain

import (
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"
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

// DefaultPullRequestTypes are the types a pull_request entry without types listens to, the forge's own default.
var DefaultPullRequestTypes = []PullRequestType{PROpened, PRSynchronize, PRReopened}

const maxNameLen = 80

var (
	ErrBadTaskSpec = errors.New("bad task spec")
	inputName      = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_-]*$`)
)

// ParseSpecName accepts the name a person gave a spec, which is shown as written: one line of at most
// 80 characters, with no space around it and no control character.
func ParseSpecName(s string) (SpecName, error) {
	if s == "" || s != strings.TrimSpace(s) || utf8.RuneCountInString(s) > maxNameLen || strings.ContainsFunc(s, unicode.IsControl) {
		return "", fmt.Errorf("%w: name %q is not one line of 1 to %d characters without space around it", ErrBadTaskSpec, s, maxNameLen)
	}
	return SpecName(s), nil
}

func CheckInputName(s string) error {
	if !inputName.MatchString(s) {
		return fmt.Errorf("%w: input name %q is not letters, digits, - and _", ErrBadTaskSpec, s)
	}
	return nil
}

func ParsePullRequestType(s string) (PullRequestType, error) {
	if t := PullRequestType(s); knownPullRequestTypes[t] {
		return t, nil
	}
	return "", fmt.Errorf("%w: unknown pull_request type %q", ErrBadTaskSpec, s)
}

// PullRequestOn starts a task on pull request activity. No types means DefaultPullRequestTypes.
type PullRequestOn struct {
	Types []PullRequestType
}

// ScheduleOn starts a task on a cron schedule of five fields.
type ScheduleOn struct {
	Cron string
}

// Input is a run input in the shape GitHub Actions has. Nothing reads it yet.
type Input struct {
	Name        string
	Description string
	Default     *string
	Required    bool
}

// DispatchOn lets a person start a task with Run, and keeps the inputs the spec declared.
type DispatchOn struct {
	Inputs []Input
}

// On lists what starts tasks of a spec.
type On struct {
	PullRequest      *PullRequestOn
	Schedule         []ScheduleOn
	WorkflowDispatch *DispatchOn
}

// Concurrency says what a newer event does to a task of the same spec and event group. With
// CancelInProgress it supersedes the older live tasks, and without it both go on.
type Concurrency struct {
	CancelInProgress bool
}

// TaskSpec is what a task spec file says. Prompt and ReviewPrompt are templates, filled by Fill, and an
// empty ReviewPrompt means no review step. Repositories narrows the repositories whose events apply,
// and empty means every allowed repository.
type TaskSpec struct {
	Name         SpecName
	Repositories []Repo
	On           On
	Concurrency  Concurrency
	Prompt       string
	ReviewPrompt string
}

// Validate refuses a spec that could not start a task: it needs a one-line name, at least one way to
// start, a prompt, whole and distinct repositories, known and distinct pull request types, five-field
// crons that can fire, distinct inputs, and prompts whose expressions are supported.
func (s TaskSpec) Validate() error {
	if _, err := ParseSpecName(string(s.Name)); err != nil {
		return err
	}
	if s.On.PullRequest == nil && len(s.On.Schedule) == 0 && s.On.WorkflowDispatch == nil {
		return fmt.Errorf("%w: %s has nothing in on", ErrBadTaskSpec, s.Name)
	}
	if strings.TrimSpace(s.Prompt) == "" {
		return fmt.Errorf("%w: %s has no prompt", ErrBadTaskSpec, s.Name)
	}
	for _, check := range []func() error{s.validateRepositories, s.validatePrompts, s.On.validate} {
		if err := check(); err != nil {
			return fmt.Errorf("%w: %s: %w", ErrBadTaskSpec, s.Name, err)
		}
	}
	return nil
}

func (s TaskSpec) validateRepositories() error {
	seen := make(map[Repo]bool, len(s.Repositories))
	for _, r := range s.Repositories {
		if !r.Complete() || seen[r] {
			return fmt.Errorf("incomplete or repeated repository %q", r)
		}
		seen[r] = true
	}
	return nil
}

func (s TaskSpec) validatePrompts() error {
	if err := CheckTemplate(s.Prompt); err != nil {
		return fmt.Errorf("prompt: %w", err)
	}
	if err := CheckTemplate(s.ReviewPrompt); err != nil {
		return fmt.Errorf("review: %w", err)
	}
	return nil
}

func (o On) validate() error {
	if err := o.PullRequest.validate(); err != nil {
		return err
	}
	for _, sc := range o.Schedule {
		if err := CheckCron(sc.Cron); err != nil {
			return err
		}
	}
	return o.WorkflowDispatch.validate()
}

func (p *PullRequestOn) validate() error {
	if p == nil {
		return nil
	}
	seen := make(map[PullRequestType]bool, len(p.Types))
	for _, typ := range p.Types {
		if !knownPullRequestTypes[typ] || seen[typ] {
			return fmt.Errorf("unknown or repeated pull_request type %q", typ)
		}
		seen[typ] = true
	}
	return nil
}

func (d *DispatchOn) validate() error {
	if d == nil {
		return nil
	}
	seen := make(map[string]bool, len(d.Inputs))
	for _, in := range d.Inputs {
		if CheckInputName(in.Name) != nil || seen[in.Name] {
			return fmt.Errorf("invalid or repeated input name %q", in.Name)
		}
		seen[in.Name] = true
	}
	return nil
}

// Prompts are a spec's prompt and review prompt with their expressions filled in.
type Prompts struct {
	Prompt       string
	ReviewPrompt string
}

// Fill fills both prompts from the event's raw JSON payload and the run inputs.
func (s TaskSpec) Fill(payload json.RawMessage, inputs map[string]string) (Prompts, error) {
	prompt, err := FillTemplate(s.Prompt, payload, inputs)
	if err != nil {
		return Prompts{}, fmt.Errorf("prompt: %w", err)
	}
	review, err := FillTemplate(s.ReviewPrompt, payload, inputs)
	if err != nil {
		return Prompts{}, fmt.Errorf("review: %w", err)
	}
	return Prompts{Prompt: prompt, ReviewPrompt: review}, nil
}
