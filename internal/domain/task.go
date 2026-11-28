package domain

import (
	"encoding/json"
	"time"
)

type (
	TaskID      string
	ProjectName string
	SkillName   string
	Person      string
	Commit      string
	Dir         string
	RunnerID    string
	RepoHost    string
	RepoPath    string
	EventSource string
	EventGroup  string
	Revision    string
)

// Repo names a repository by its host and its path there, never by a bare path.
type Repo struct {
	Host RepoHost
	Path RepoPath
}

// Event is the integration event that started a task. Group and Context are opaque to the core.
type Event struct {
	Source  EventSource
	Group   EventGroup
	Rev     Revision
	Context json.RawMessage
}

// Spec names the task spec that started a task and its file in the team's home, both required.
type Spec struct {
	Name SpecName
	File string
}

type Task struct {
	ID      TaskID
	Project ProjectName
	Prompt  string
	// ReviewPrompt is sent after the first publish. Empty means the task has no review step.
	ReviewPrompt string
	StartedBy    string
	// Spec and Event are nil on a plain-prompt task. A person who runs a spec by hand starts a task with both.
	Spec    *Spec
	Event   *Event
	State   State
	Version int64
	Pins    map[Dir]Commit
	// Refs are the refs a person named for entries, by dir. Pins hold what they resolved to.
	Refs      map[Dir]string
	Runner    RunnerID
	Run       int64
	Gen       int64
	LastSeq   int64
	Deadline  time.Time
	KeepUntil time.Time
	Failure   string
	// TokenOwner is the fallback person on a task an event started.
	TokenOwner Person
	CreatedAt  time.Time
	EndedAt    time.Time
}

// Answers tells whether the task posts its result back to its event, which it does when the event's source is answerable.
func (t Task) Answers(answerable map[EventSource]bool) bool {
	return t.Event != nil && answerable[t.Event.Source]
}

func (t Task) Flow(answerable map[EventSource]bool) Flow {
	switch {
	case t.Event == nil:
		return Plain
	case t.Answers(answerable):
		return Answering
	}
	return Evented
}

// TaskKey is unique among tasks an event started, and each spec that hears an event has its own.
type TaskKey struct {
	Project ProjectName
	Spec    SpecName
	Source  EventSource
	Group   EventGroup
	Rev     Revision
}

func (t Task) Key() (TaskKey, bool) {
	if t.Event == nil || t.Spec == nil {
		return TaskKey{}, false
	}
	return TaskKey{Project: t.Project, Spec: t.Spec.Name, Source: t.Event.Source, Group: t.Event.Group, Rev: t.Event.Rev}, true
}
