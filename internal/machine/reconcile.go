package machine

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/cenkalti/backoff/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/wire"
)

// reconcile makes what the runner holds match what the store wants: a task the portal no longer
// wants is stopped, and a task still starting on this machine is started again. A cancel on any
// replica reaches the runner this way, on the holder's next tick.
func (h *holding) reconcile(ctx context.Context) error {
	var listed wire.TaskListReply
	if err := call(ctx, h.link, wire.KindTaskList, wire.TaskList{}, &listed); err != nil {
		return err
	}
	var errs []error
	for _, lt := range listed.Tasks {
		errs = append(errs, h.keepOrStop(ctx, lt.Fence))
	}
	on, err := h.c.cfg.Store.TasksOn(ctx, h.id)
	if err != nil {
		return errors.Join(append(errs, fmt.Errorf("tasks on the machine: %w", err))...)
	}
	for _, t := range on {
		if t.State == domain.Starting {
			h.launch(ctx, t)
		}
	}
	return errors.Join(errs...)
}

func (h *holding) keepOrStop(ctx context.Context, f wire.Fence) error {
	t, err := h.c.cfg.Store.Load(ctx, domain.TaskID(f.Task))
	switch {
	case err != nil && !errors.Is(err, domain.ErrNotFound):
		return fmt.Errorf("load task %s: %w", f.Task, err)
	case err == nil && t.Runner == h.id && keeps(t, h.c.cfg.Clock.Now()):
		return nil
	}
	h.log.Info("stopping a task the portal no longer wants", "task", f.Task, "run", f.Run)
	return call(ctx, h.link, wire.KindTaskStop, wire.TaskStop{Fence: f}, nil)
}

// watch follows task.watch for the life of the link, opening it again after a drop.
func (h *holding) watch(ctx context.Context) {
	b := backoff.NewExponentialBackOff()
	b.MaxInterval = 5 * time.Second
	_, _ = backoff.Retry(ctx, func() (struct{}, error) {
		err := h.follow(ctx)
		select {
		case <-h.link.Done():
			return struct{}{}, backoff.Permanent(err)
		default:
		}
		return struct{}{}, err
	}, backoff.WithBackOff(b), backoff.WithMaxElapsedTime(0))
}

func (h *holding) follow(ctx context.Context) error {
	st, err := h.link.Open(ctx, wire.KindTaskWatch, wire.TaskWatch{}, nil)
	if err != nil {
		return fmt.Errorf("open task.watch: %w", err)
	}
	defer func() { _ = st.Close() }()
	for {
		var l wire.TaskWatchLine
		if err := st.Recv(&l); err != nil {
			return fmt.Errorf("task.watch: %w", err)
		}
		h.line(ctx, l)
	}
}

// line turns a session line into a report, and the state machine decides what it moves. A live
// session means the desk, Box and proxy are up; a finished one means the harness exited.
func (h *holding) line(ctx context.Context, l wire.TaskWatchLine) {
	if l.Part != wire.SessionPart {
		return
	}
	kind := task.PartsUp
	if l.State == wire.SessionFinished {
		kind = task.HarnessExited
	}
	h.report(ctx, domain.TaskID(l.Task), task.Report{Kind: kind, Run: l.Run})
}
