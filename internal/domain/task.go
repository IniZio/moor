package domain

import (
	"encoding/json"
	"time"
)

type (
	TaskID        string
	ProjectName   string
	RecipeName    string
	SkillName     string
	Person        string
	Commit        string
	Dir           string
	RunnerID      string
	RepoHost      string
	RepoPath      string
	TriggerSource string
	TriggerGroup  string
	Revision      string
)

// Repo names a repository by its host and its path there, never by a bare path.
type Repo struct {
	Host RepoHost
	Path RepoPath
}

// Trigger is the integration event that started a task. Group and Context are opaque to the core.
type Trigger struct {
	Source  TriggerSource
	Group   TriggerGroup
	Rev     Revision
	Context json.RawMessage
}

type Task struct {
	ID      TaskID
	Project ProjectName
	Prompt  string
	// Kind is empty on a free-prompt task.
	Kind      RecipeName
	StartedBy string
	Trigger   *Trigger
	State     State
	Version   int64
	Pins      map[Dir]Commit
	// Refs are the refs a person named for entries, by dir. Pins hold what they resolved to.
	Refs      map[Dir]string
	Runner    RunnerID
	Run       int64
	Gen       int64
	LastSeq   int64
	Deadline  time.Time
	KeepUntil time.Time
	Failure   string
	// TokenOwner is the fallback person on a task a trigger started.
	TokenOwner Person
	CreatedAt  time.Time
	EndedAt    time.Time
}

func (t Task) Flow() Flow {
	if t.Trigger == nil {
		return FreePrompt
	}
	return Triggered
}

// TaskKey is unique among tasks a trigger started, so a redelivered event starts nothing.
type TaskKey struct {
	Project ProjectName
	Source  TriggerSource
	Group   TriggerGroup
	Rev     Revision
}

func (t Task) Key() (TaskKey, bool) {
	if t.Trigger == nil {
		return TaskKey{}, false
	}
	return TaskKey{Project: t.Project, Source: t.Trigger.Source, Group: t.Trigger.Group, Rev: t.Trigger.Rev}, true
}
