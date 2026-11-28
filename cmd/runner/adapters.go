package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"strconv"

	"github.com/IniZio/moor/internal/adapter/docker"
	"github.com/IniZio/moor/internal/adapter/herdr"
	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

// dialer makes one link to the portal and sorts its failures into what the runner does next.
func dialer(cfg link.AgentConfig) func(ctx context.Context) (runner.Link, error) {
	return func(ctx context.Context) (runner.Link, error) {
		c, _, err := link.Dial(ctx, cfg)
		var refused *link.RefusedError
		var update *link.UpdateRequiredError
		switch {
		case errors.Is(err, link.ErrRelinkLater):
			return nil, fmt.Errorf("%w: %w", runner.ErrRelink, err)
		case errors.As(err, &refused), errors.As(err, &update):
			return nil, fmt.Errorf("%w: %w", runner.ErrFatal, err)
		case err != nil:
			return nil, fmt.Errorf("dial: %w", err)
		}
		s, err := link.NewSession(ctx, c, true)
		if err != nil {
			return nil, fmt.Errorf("dial: %w", err)
		}
		return session{s}, nil
	}
}

type session struct{ *link.Session }

func (s session) Accept(ctx context.Context) (runner.Stream, error) {
	st, err := s.Session.Accept(ctx)
	if err != nil {
		return nil, fmt.Errorf("accept: %w", err)
	}
	return st, nil
}

// host is the Docker adapter seen as the runner's host. The fence and the key are the same three
// numbers under two names.
type host struct{ d *docker.Docker }

func keyOf(f wire.Fence) docker.Key {
	return docker.Key{Task: domain.TaskID(f.Task), Run: f.Run, Gen: f.Gen}
}

func (h host) Up(ctx context.Context, b runner.Box) error {
	spec := docker.Spec{Key: keyOf(b.Fence), Portal: b.Proxies.Portal, BoxToken: b.Proxies.BoxToken, DeskToken: b.Proxies.DeskToken}
	if err := h.d.Up(ctx, spec); err != nil {
		return fmt.Errorf("up: %w", err)
	}
	return nil
}

func (h host) Down(ctx context.Context, f wire.Fence) error {
	if err := h.d.Down(ctx, keyOf(f)); err != nil {
		return fmt.Errorf("down: %w", err)
	}
	return nil
}

func (h host) List(ctx context.Context) ([]runner.Held, error) {
	tasks, err := h.d.List(ctx)
	if err != nil {
		return nil, fmt.Errorf("list: %w", err)
	}
	held := make([]runner.Held, len(tasks))
	for i, t := range tasks {
		held[i].Fence = wire.Fence{Task: string(t.Key.Task), Run: t.Key.Run, Gen: t.Key.Gen}
		for _, c := range t.Containers {
			held[i].Parts = append(held[i].Parts, runner.Part{Name: string(c.Part), Container: c.Name, State: c.State})
		}
	}
	return held, nil
}

// attach runs argv in the task's desk under a PTY on this host.
func (h host) attach(ctx context.Context, task string, s wire.Size, argv ...string) (runner.TTY, error) {
	t, err := h.d.ExecTTY(ctx, domain.TaskID(task), docker.Desk, s.Cols, s.Rows, argv...)
	if err != nil {
		return nil, fmt.Errorf("attach to the desk: %w", err)
	}
	return tty{t}, nil
}

type tty struct{ *docker.TTY }

func (t tty) Resize(s wire.Size) error {
	if err := t.TTY.Resize(s.Cols, s.Rows); err != nil {
		return fmt.Errorf("tty: %w", err)
	}
	return nil
}

func (h host) connect(ctx context.Context, task string) (io.ReadWriteCloser, error) {
	rwc, err := h.d.Exec(ctx, domain.TaskID(task), docker.Desk, "hc", "connect")
	if err != nil {
		return nil, fmt.Errorf("connect to the desk: %w", err)
	}
	return rwc, nil
}

// sessionName is the herdr-core terminal of a task's harness session in its desk.
const sessionName = "task"

// sessions runs the harness session in herdr-core in the desk. connect opens a fresh stream to the
// desk's herdr-core, and attach runs a command in the desk under a PTY.
type sessions struct {
	connect func(ctx context.Context, task string) (io.ReadWriteCloser, error)
	attach  func(ctx context.Context, task string, s wire.Size, argv ...string) (runner.TTY, error)
	argv    []string
}

// Attach runs hc attach on the session in the desk, with --observe for a watcher, so herdr-core
// never takes a watcher's keys or size.
func (s sessions) Attach(ctx context.Context, a wire.TermAttach) (runner.TTY, error) {
	argv := []string{"hc", "attach", sessionName, "--history", strconv.Itoa(a.History)}
	if a.Mode == wire.TermObserve {
		argv = append(argv, "--observe")
	}
	return s.attach(ctx, a.Task, wire.Size{Cols: a.Cols, Rows: a.Rows}, argv...)
}

func (s sessions) Screen(ctx context.Context, task string, lines int) (string, error) {
	c, err := s.dial(ctx, task)
	if err != nil {
		return "", err
	}
	defer func() { _ = c.Close() }()
	r, err := c.Read(ctx, sessionName, lines)
	if err != nil {
		return "", fmt.Errorf("read the session of %s: %w", task, err)
	}
	return r.Text, nil
}

func (s sessions) Type(ctx context.Context, task, text string) error {
	c, err := s.dial(ctx, task)
	if err != nil {
		return err
	}
	defer func() { _ = c.Close() }()
	if err := c.Send(ctx, sessionName, text); err != nil {
		return fmt.Errorf("type into the session of %s: %w", task, err)
	}
	return nil
}

func (s sessions) dial(ctx context.Context, task string) (*herdr.Conn, error) {
	rwc, err := s.connect(ctx, task)
	if err != nil {
		return nil, err
	}
	c := herdr.Dial(rwc)
	if err := c.Hello(ctx); err != nil {
		_ = c.Close()
		return nil, fmt.Errorf("session of %s: %w", task, err)
	}
	return c, nil
}

// Start runs the harness with the task's prompt in its environment. A session already live under
// the name converges: herdr-core refuses the second one as name_taken.
func (s sessions) Start(ctx context.Context, t wire.TaskStart) error {
	c, err := s.dial(ctx, t.Task)
	if err != nil {
		return err
	}
	defer func() { _ = c.Close() }()
	_, err = c.Run(ctx, herdr.RunReq{
		Argv: s.argv, Name: sessionName, Cwd: "/workspace", NoParent: true,
		Env:    map[string]string{"TASK_PROMPT": t.Prompt},
		Labels: map[string]string{"moor.task": t.Task, "moor.run": strconv.FormatInt(t.Run, 10)},
	})
	if err != nil && !herdr.IsKind(err, herdr.KindNameTaken) {
		return fmt.Errorf("start the session of %s: %w", t.Task, err)
	}
	return nil
}

// Watch reports the session now and after every herdr-core event in the desk. A subscribed stream
// answers nothing else, so the state is read on a second one.
func (s sessions) Watch(ctx context.Context, task string, on func(runner.SessionState)) error {
	c, err := s.dial(ctx, task)
	if err != nil {
		return err
	}
	defer func() { _ = c.Close() }()
	feed, err := s.connect(ctx, task)
	if err != nil {
		return err
	}
	var last *runner.SessionState
	report := func() {
		t, err := c.Get(ctx, sessionName)
		if err != nil {
			return
		}
		if now := stateOf(t); last == nil || *last != now {
			last = &now
			on(now)
		}
	}
	err = herdr.Subscribe(ctx, feed, nil, func([]herdr.Terminal) { report() }, func(herdr.Event) { report() })
	return fmt.Errorf("watch the session of %s: %w", task, err)
}

func stateOf(t herdr.Terminal) runner.SessionState {
	switch {
	case !t.Live():
		return runner.SessionState{State: wire.SessionFinished, Code: t.Code()}
	case t.Status != nil:
		return runner.SessionState{State: t.Status.State}
	}
	return runner.SessionState{State: "live"}
}
