package task

import (
	"context"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// Store keeps tasks, their events and their result versions. It speaks only domain types, so an
// adapter can implement it without importing this package.
type Store interface {
	// Load and ByKey return domain.ErrNotFound for no row.
	Load(ctx context.Context, id domain.TaskID) (domain.Task, error)
	ByKey(ctx context.Context, key domain.TaskKey) (domain.Task, error)
	// LiveInGroup lists the live tasks of one repository, spec, event source and event group, oldest first.
	LiveInGroup(ctx context.Context, repo domain.Repo, spec domain.SpecName, source domain.EventSource, group domain.EventGroup) ([]domain.Task, error)
	// Versions lists a task's result versions in order.
	Versions(ctx context.Context, id domain.TaskID) ([]domain.ResultVersion, error)
	// HasNote tells whether the task has a note row with exactly this text.
	HasNote(ctx context.Context, id domain.TaskID, text string) (bool, error)
	// Save applies w in one transaction. It returns the task as stored, with its version bumped.
	Save(ctx context.Context, w domain.TaskWrite) (domain.Task, error)
}

type Clock interface {
	Now() time.Time
}

// Config holds the fixed product limits. A zero duration means no limit.
type Config struct {
	TimeLimit  time.Duration
	KeepWindow time.Duration
	// Answerable lists the event sources whose tasks post their result back, so they go through
	// delivering. A task of any other source, and a plain-prompt task, is done on its accepted publish.
	Answerable map[domain.EventSource]bool
	// Attempts caps the reads and writes an operation repeats when it loses a race. Zero means 5.
	Attempts int
}

// EffectKind is something the caller must do after the write, which this package cannot.
type EffectKind uint8

const (
	// StopJob asks the runner to remove the task's desk, Box and proxy.
	StopJob EffectKind = iota + 1
	// SendReviewTurn asks the session to send the task's review prompt.
	SendReviewTurn
	// Deliver asks the bridge to create or edit the task's delivery for Version.
	Deliver
)

// Effect is derived from the task's state, so a repeated call reports it again until the caller acts.
type Effect struct {
	Kind    EffectKind
	Task    domain.TaskID
	Runner  domain.RunnerID
	Run     int64
	Gen     int64
	Version int
}

// Outcome is what one operation did. No Events means the call changed nothing.
type Outcome struct {
	Task    domain.Task
	Events  []domain.EventRow
	Effects []Effect
}

func (o *Outcome) merge(other Outcome) {
	o.Events = append(o.Events, other.Events...)
	o.Effects = append(o.Effects, other.Effects...)
}
