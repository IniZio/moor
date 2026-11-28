package runner

import (
	"context"
	"io"

	"github.com/IniZio/moor/internal/wire"
)

// Held is one task as the host's container labels show it. It is everything the runner knows
// about a task across a restart: the fence of its newest launch and its parts.
type Held struct {
	Fence wire.Fence
	Parts []Part
}

// Part is one container of a task. Name is the part, such as box or desk. State is the host's
// word for the container, such as running or exited.
type Part struct {
	Name      string
	Container string
	State     string
}

// Box is what Up needs for one launch: the fence, and how the task's proxies reach the portal.
type Box struct {
	Fence   wire.Fence
	Proxies wire.ProxyAccess
}

// Host starts, finds and removes the containers of tasks. Up and Down converge, so a repeat is
// safe. Down of a task the host does not have succeeds.
type Host interface {
	Up(ctx context.Context, b Box) error
	Down(ctx context.Context, f wire.Fence) error
	List(ctx context.Context) ([]Held, error)
}

// SessionState is what the harness session of a task last reported. State is finished once the
// harness exited, with Code its exit code. Before that it is the session's status, or live.
type SessionState struct {
	State string
	Code  int
}

// Sessions starts and follows the harness session in a task's desk. Start of a session that is
// already live succeeds. Watch calls on with the session's state now and on each change, until ctx
// ends or the session cannot be reached. Attach puts one more viewer on the session: closing its TTY
// ends that viewer and never the session. Screen reads the screen as text with lines of scrollback,
// and Type types text and presses enter.
type Sessions interface {
	Start(ctx context.Context, s wire.TaskStart) error
	Watch(ctx context.Context, task string, on func(SessionState)) error
	Attach(ctx context.Context, a wire.TermAttach) (TTY, error)
	Screen(ctx context.Context, task string, lines int) (string, error)
	Type(ctx context.Context, task, text string) error
}

// TTY is one viewer's terminal on a session: its output, its keys and its size.
type TTY interface {
	io.ReadWriteCloser
	Resize(s wire.Size) error
}

// Link is one live link to the portal. Accept returns the portal's next op, and fails once the
// link is gone.
type Link interface {
	Accept(ctx context.Context) (Stream, error)
	Close() error
}

// Stream is one op the portal opened.
type Stream interface {
	Op() wire.Kind
	Decode(v any) error
	Reply(body any) error
	Fail(e *wire.Error) error
	Send(v any) error
	Recv(v any) error
	// Read and Write carry the raw bytes that follow the reply, such as term.attach frames.
	io.ReadWriter
	Close() error
}
