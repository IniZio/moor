package domain

import (
	"encoding/json"
	"time"
)

type (
	TaskID        string
	ProjectName   string
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

// Trigger is the integration event that started a task, and the definition that took it. Group and Context are opaque to the core.
type Trigger struct {
	Name    SpecName
	Source  TriggerSource
	Group   TriggerGroup
	Rev     Revision
	Context json.RawMessage
}

type Task struct {
	ID      TaskID
	Project ProjectName
	Prompt  string
	// ReviewPrompt is sent after the first publish. Empty means the task has no review step.
	ReviewPrompt string
	// Reply is where the result is posted. Empty means the task ends on the accepted publish.
	Reply     ReplyTarget
	StartedBy string
	// Trigger is nil on a task a person started.
	Trigger *Trigger
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
	// TokenOwner is the fallback person on a task a trigger started.
	TokenOwner Person
	CreatedAt  time.Time
	EndedAt    time.Time
}

func (t Task) Flow() Flow {
	return Flow{Triggered: t.Trigger != nil, Replies: t.Reply.Set()}
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
