package machine

import (
	"context"
	"io"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/wire"
)

// Store keeps machines and finds the tasks placement needs.
type Store interface {
	AddMachine(ctx context.Context, a domain.MachineAdd) error
	JoinMachine(ctx context.Context, j domain.MachineJoin) error
	Machines(ctx context.Context) ([]domain.Machine, error)
	Machine(ctx context.Context, id domain.RunnerID) (domain.Machine, error)
	Heard(ctx context.Context, id domain.RunnerID, gen int64, at time.Time) (bool, error)
	DrainMachine(ctx context.Context, id domain.RunnerID, at time.Time) error
	DisconnectMachine(ctx context.Context, id domain.RunnerID) error
	ResumeMachine(ctx context.Context, id domain.RunnerID) error
	RevokeMachine(ctx context.Context, id domain.RunnerID, at time.Time) error
	RemoveMachine(ctx context.Context, id domain.RunnerID) error
	WaitingTasks(ctx context.Context, limit int) ([]domain.TaskID, error)
	TasksOn(ctx context.Context, id domain.RunnerID) ([]domain.Task, error)
	Load(ctx context.Context, id domain.TaskID) (domain.Task, error)
}

// Tasks is the task service calls a runner's reports become.
type Tasks interface {
	Report(ctx context.Context, id domain.TaskID, r task.Report) (task.Outcome, error)
	MachineRevoked(ctx context.Context, id domain.TaskID) (task.Outcome, error)
}

type Clock interface {
	Now() time.Time
}

// Link is the portal's end of one runner link, already through its handshake.
type Link interface {
	Open(ctx context.Context, op wire.Kind, body, out any) (Stream, error)
	Done() <-chan struct{}
	Close() error
}

// Stream is one op the portal opened on a link. Read and Write carry the raw bytes after the reply,
// such as term.attach frames.
type Stream interface {
	Recv(v any) error
	io.ReadWriter
	Close() error
}
