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
	// LiveInGroup lists the live tasks of one project, source and group, oldest first.
	LiveInGroup(ctx context.Context, project domain.ProjectName, source domain.TriggerSource, group domain.TriggerGroup) ([]domain.Task, error)
	// Versions lists a task's result versions in order.
	Versions(ctx context.Context, id domain.TaskID) ([]domain.ResultVersion, error)
	// Save applies w in one transaction. It returns the task as stored, with its version bumped.
	Save(ctx context.Context, w domain.TaskWrite) (domain.Task, error)
}

type Clock interface {
	Now() time.Time
}

// RecipeLookup finds a recipe by the name a task records as its kind.
type RecipeLookup func(domain.RecipeName) (domain.Recipe, bool)

// Config holds the fixed product limits. A zero duration means no limit.
type Config struct {
	TimeLimit  time.Duration
	KeepWindow time.Duration
	// Attempts caps the reads and writes an operation repeats when it loses a race. Zero means 5.
	Attempts int
}

// EffectKind is something the caller must do after the write, which this package cannot.
type EffectKind uint8

const (
	// StopJob asks the runner to remove the task's desk, Box and proxy.
	StopJob EffectKind = iota + 1
	// SendReviewTurn asks the session to run the recipe's review turn.
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
