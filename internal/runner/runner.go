package runner

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"time"

	"github.com/cenkalti/backoff/v5"

	"github.com/IniZio/moor/internal/wire"
)

var (
	// ErrFatal wraps a dial error after which redialling cannot help, such as a refusal.
	ErrFatal = errors.New("runner: the portal will not take this runner")
	// ErrRelink wraps a dial error that asks for a quick redial: the portal is draining.
	ErrRelink = errors.New("runner: the portal asked for a relink")
	// ErrRevoked wraps a dial error that says a person revoked this machine. The runner then removes
	// the containers of the tasks it holds and ends.
	ErrRevoked = fmt.Errorf("runner: this machine was revoked: %w", ErrFatal)
)

const (
	relinkWait  = 200 * time.Millisecond
	maxDialWait = 10 * time.Second
	// removeAllWait bounds the removal of a revoked machine's containers.
	removeAllWait = 2 * time.Minute
)

// SessionPart is the part name of the harness session in task.watch lines.
const SessionPart = wire.SessionPart

// Config wires a runner. Dial makes one link, already through its handshake.
type Config struct {
	Dial     func(ctx context.Context) (Link, error)
	Host     Host
	Sessions Sessions
	Build    string
	// Machine names this runner in the notes it writes.
	Machine string
	Slots   int
	Log     *slog.Logger
}

// Runner serves the portal's ops for the tasks on one host.
type Runner struct {
	cfg  Config
	ctx  context.Context
	wg   sync.WaitGroup
	hub  hub
	mu   sync.Mutex
	jobs map[string]*job
}

// job is what one process adds to a task the host holds: a lock that orders its ops and the
// follower of its session. Both are rebuilt by adopt, so losing them with the process loses nothing.
type job struct {
	op       sync.Mutex
	watching wire.Fence
	stop     context.CancelFunc
	session  *wire.TaskWatchLine
	// noted is the launch whose notes are in notes.
	noted wire.Fence
	notes []wire.TaskWatchLine
}

func New(cfg Config) *Runner {
	if cfg.Log == nil {
		cfg.Log = slog.New(slog.DiscardHandler)
	}
	return &Runner{cfg: cfg, jobs: map[string]*job{}, hub: hub{subs: map[chan wire.TaskWatchLine]struct{}{}}}
}

// Run links to the portal and serves it until ctx ends, redialling with backoff whenever the link
// drops. It returns nil when ctx ends and an ErrFatal error when the portal refuses the runner. The
// task containers are never touched on the way out.
func (r *Runner) Run(ctx context.Context) error {
	r.ctx = ctx
	defer r.wg.Wait()
	b := backoff.NewExponentialBackOff()
	b.MaxInterval = maxDialWait
	_, err := backoff.Retry(ctx, func() (struct{}, error) { return struct{}{}, r.linkOnce(ctx) },
		backoff.WithBackOff(b), backoff.WithMaxElapsedTime(0),
		backoff.WithNotify(func(err error, next time.Duration) { r.cfg.Log.Warn("link down", "err", err, "retry_in", next) }))
	r.unwatchAll()
	if ctx.Err() != nil || err == nil {
		return nil
	}
	if errors.Is(err, ErrRevoked) {
		err = errors.Join(err, r.removeAll(ctx))
	}
	return fmt.Errorf("runner: %w", err)
}

// removeAll takes down every task the host holds. A revoked machine's tasks are failed, so nothing
// of theirs is kept for a person to look at, and this runner is the only thing that can remove them.
func (r *Runner) removeAll(ctx context.Context) error {
	ctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), removeAllWait)
	defer cancel()
	held, err := r.list(ctx)
	if err != nil {
		return err
	}
	var errs []error
	for _, h := range held {
		j := r.job(h.Fence.Task)
		j.op.Lock()
		r.unwatch(h.Fence.Task)
		if err := r.cfg.Host.Down(ctx, h.Fence); err != nil {
			errs = append(errs, fmt.Errorf("runner: down %s: %w", h.Fence.Task, err))
		}
		j.op.Unlock()
	}
	return errors.Join(errs...)
}

// linkOnce dials, re-adopts what the host holds and serves ops until the link drops. A link that
// was up and dropped is redialled at once with the backoff reset.
func (r *Runner) linkOnce(ctx context.Context) error {
	l, err := r.cfg.Dial(ctx)
	switch {
	case errors.Is(err, ErrFatal):
		return &backoff.PermanentError{Err: err}
	case errors.Is(err, ErrRelink):
		return &backoff.RetryAfterError{Duration: relinkWait}
	case err != nil:
		return err
	}
	defer func() { _ = l.Close() }()
	if err := r.adopt(ctx); err != nil {
		return err
	}
	for {
		st, err := l.Accept(ctx)
		if err != nil {
			r.cfg.Log.Info("link dropped", "err", err)
			return &backoff.RetryAfterError{Duration: relinkWait}
		}
		r.wg.Go(func() { r.serve(st) })
	}
}

// adopt follows the session of every task the host holds, by label alone, and ends the viewers a
// runner before this link left in their desks. No viewer of this link exists yet, and those of the
// last one went with its streams.
func (r *Runner) adopt(ctx context.Context) error {
	held, err := r.list(ctx)
	if err != nil {
		return err
	}
	for _, h := range held {
		r.watch(h.Fence)
		if err := r.cfg.Sessions.ClearViewers(ctx, h.Fence.Task); err != nil {
			r.cfg.Log.Warn("clear stale viewers", "task", h.Fence.Task, "err", err)
		}
	}
	r.cfg.Log.Info("linked", "tasks", len(held))
	return nil
}

func (r *Runner) job(task string) *job {
	r.mu.Lock()
	defer r.mu.Unlock()
	j := r.jobs[task]
	if j == nil {
		j = &job{}
		r.jobs[task] = j
	}
	return j
}

// held is the task as the host shows it, false when the host has none of its containers.
func (r *Runner) held(ctx context.Context, task string) (Held, bool, error) {
	all, err := r.list(ctx)
	if err != nil {
		return Held{}, false, err
	}
	for _, h := range all {
		if h.Fence.Task == task {
			return h, true, nil
		}
	}
	return Held{}, false, nil
}

func (r *Runner) list(ctx context.Context) ([]Held, error) {
	held, err := r.cfg.Host.List(ctx)
	if err != nil {
		return nil, fmt.Errorf("runner: list the host's tasks: %w", err)
	}
	return held, nil
}
