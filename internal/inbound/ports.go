package inbound

import (
	"context"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// Tx is the store inside one transaction: the task store the task service reads and writes through, and the
// rows of a delivery.
type Tx interface {
	task.Store
	NextTaskID(ctx context.Context) (domain.TaskID, error)
	// RecordInbound stores a delivery and says whether it was the first of its id.
	RecordInbound(ctx context.Context, ev domain.InboundEvent) (bool, error)
	LinkInbound(ctx context.Context, source domain.EventSource, delivery string, id domain.TaskID) error
	FinishInbound(ctx context.Context, source domain.EventSource, delivery, outcome string) error
	// Inbound reads a stored delivery and the tasks it started.
	Inbound(ctx context.Context, source domain.EventSource, delivery string) (domain.InboundEvent, []domain.TaskID, error)
}

// Store runs fn in one transaction, which commits when fn returns nil.
type Store interface {
	InTx(ctx context.Context, fn func(tx Tx) error) error
}

// Specs lists the task specs of the team's home.
type Specs interface {
	TaskSpecs(ctx context.Context) ([]domain.SpecFile, error)
}

// Allowed lists the repositories the newest settings allow.
type Allowed interface {
	Allowed(ctx context.Context) ([]domain.Repo, error)
}

// Linker reads a link a person pasted into the event it stands for. It returns domain.ErrBadLink, wrapped, for text
// that is not such a link.
type Linker interface {
	Occurrence(ctx context.Context, link string) (domain.Occurrence, error)
}

// Starter is the task service a Run starts its one task through, and the numbers it takes.
type Starter interface {
	StartByEvent(ctx context.Context, in task.EventInput) (task.Outcome, error)
}

// Numbers hands out task ids.
type Numbers interface {
	NextTaskID(ctx context.Context) (domain.TaskID, error)
}
