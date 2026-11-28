package domain

import (
	"encoding/json"
	"time"
)

type (
	TaskID      string
	SkillName   string
	Person      string
	Commit      string
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

// Complete is true when both the host and the path are set.
func (r Repo) Complete() bool { return r.Host != "" && r.Path != "" }

func (r Repo) String() string { return string(r.Host) + "/" + string(r.Path) }

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
	ID     TaskID
	Repo   Repo
	Prompt string
	// ReviewPrompt is sent after the first publish. Empty means the task has no review step.
	ReviewPrompt string
	StartedBy    string
	// Spec and Event are nil on a plain-prompt task. A person who runs a spec by hand starts a task with both.
	Spec  *Spec
	Event *Event
	// HomeCommit is the team's home commit the task's spec was read at, empty when the home has none.
	HomeCommit Commit
	State      State
	Version    int64
	// Ref and Commit are what the task checks out, from its event: the ref fetched first and the commit. Both are
	// empty on a task that checks out its repository's default branch.
	Ref       string
	Commit    Commit
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
	Repo   Repo
	Spec   SpecName
	Source EventSource
	Group  EventGroup
	Rev    Revision
}

func (t Task) Key() (TaskKey, bool) {
	if t.Event == nil || t.Spec == nil {
		return TaskKey{}, false
	}
	return TaskKey{Repo: t.Repo, Spec: t.Spec.Name, Source: t.Event.Source, Group: t.Event.Group, Rev: t.Event.Rev}, true
}
