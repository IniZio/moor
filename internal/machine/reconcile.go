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
// wants is stopped, a task whose keep window ended is released, and a task still starting on this
// machine is started again. A cancel on any replica reaches the runner this way, on the holder's next tick.
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

// keepOrStop decides what the runner does with a task it holds, from the stored task and the clock alone, so
// a restart of any replica loses no timer. A done or failed task keeps its containers until its keep window
// ends, then the runner is told to release them, after the event that says so is recorded: a release that did
// not reach the runner is sent again on the next tick and records nothing twice.
func (h *holding) keepOrStop(ctx context.Context, f wire.Fence) error {
	t, err := h.c.cfg.Store.Load(ctx, domain.TaskID(f.Task))
	if err != nil && !errors.Is(err, domain.ErrNotFound) {
		return fmt.Errorf("load task %s: %w", f.Task, err)
	}
	now := h.c.cfg.Clock.Now()
	v := verdictOf(t, err == nil && t.Runner == h.id, now)
	if v == keep {
		return h.timeLimit(ctx, t, now)
	}
	if v == release {
		return h.release(ctx, f)
	}
	h.log.Info("stopping a task the portal no longer wants", "task", f.Task, "run", f.Run)
	return call(ctx, h.link, wire.KindTaskStop, wire.TaskStop{Fence: f}, nil)
}

// timeLimit fails a running task whose deadline has passed. Its containers stay for the keep window, as a failed
// task's do. A task the service will not fail, such as one a publish ended meanwhile, is left as it is.
func (h *holding) timeLimit(ctx context.Context, t domain.Task, now time.Time) error {
	if t.State != domain.Running || t.Deadline.IsZero() || now.Before(t.Deadline) {
		return nil
	}
	if _, err := h.c.cfg.Tasks.TimeLimit(ctx, t.ID); err != nil && !errors.Is(err, domain.ErrRefused) {
		return fmt.Errorf("fail %s at its time limit: %w", t.ID, err)
	}
	h.log.Info("failed a task at its time limit", "task", t.ID)
	return nil
}

// release records that the keep window ended and tells the runner to remove the task's containers. A
// record the task service refuses means a person reopened the task meanwhile, so nothing is removed.
func (h *holding) release(ctx context.Context, f wire.Fence) error {
	if _, err := h.c.cfg.Tasks.Released(ctx, domain.TaskID(f.Task)); err != nil {
		if errors.Is(err, domain.ErrRefused) {
			return nil
		}
		return fmt.Errorf("record the release of %s: %w", f.Task, err)
	}
	h.log.Info("releasing a task whose keep window ended", "task", f.Task, "run", f.Run)
	return call(ctx, h.link, wire.KindTaskRelease, wire.TaskRelease{Task: f.Task}, nil)
}

type verdict uint8

const (
	// keep leaves the task's containers alone.
	keep verdict = iota
	// release removes the containers of a done or failed task after its keep window.
	release
	// stop removes the containers of a task the portal has no more use for: one that is gone, on another
	// machine, cancelled or superseded.
	stop
)

// verdictOf is what the portal wants of the containers a machine holds for t. mine says t is on this machine.
// A task with no keep window to end is kept for as long as it is done or failed.
func verdictOf(t domain.Task, mine bool, now time.Time) verdict {
	switch {
	case !mine:
		return stop
	case t.State.Live():
		return keep
	case t.State == domain.Done || t.State == domain.Failed:
		if t.KeepUntil.IsZero() || now.Before(t.KeepUntil) {
			return keep
		}
		return release
	}
	return stop
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
	switch {
	case l.Part == wire.NotePart:
		h.report(ctx, domain.TaskID(l.Task), task.Report{Kind: task.Noted, Run: l.Run, Text: l.Text})
	case l.Part == wire.SessionPart && l.State == wire.SessionFinished:
		h.report(ctx, domain.TaskID(l.Task), task.Report{Kind: task.HarnessExited, Run: l.Run})
	case l.Part == wire.SessionPart:
		h.report(ctx, domain.TaskID(l.Task), task.Report{Kind: task.PartsUp, Run: l.Run})
	}
}
