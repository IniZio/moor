package machine

import (
	"context"
	"errors"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/wire"
)

// place offers waiting tasks, oldest first, to a machine that takes new tasks and has free slots.
// The claim is the task service's slot-taken report: its write checks the task's version under a row
// lock, so when two replicas claim one task the state machine refuses the second.
func (h *holding) place(ctx context.Context) error {
	m, err := h.c.cfg.Store.Machine(ctx, h.id)
	if err != nil {
		return fmt.Errorf("read machine: %w", err)
	}
	free := h.slots - m.Running
	if !m.DrainingSince.IsZero() || !m.RevokedAt.IsZero() || free <= 0 {
		return nil
	}
	ids, err := h.c.cfg.Store.WaitingTasks(ctx, free)
	if err != nil {
		return fmt.Errorf("waiting tasks: %w", err)
	}
	for _, id := range ids {
		out, err := h.c.cfg.Tasks.Report(ctx, id, task.Report{Kind: task.SlotTaken, Runner: h.id})
		if err != nil || len(out.Events) == 0 {
			h.log.Debug("not claimed", "task", id, "err", err)
			continue
		}
		h.launch(ctx, out.Task)
	}
	return nil
}

// launch starts the task on the runner unless a start of it is already in flight from this link.
func (h *holding) launch(ctx context.Context, t domain.Task) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.starting[t.ID] {
		return
	}
	h.starting[t.ID] = true
	h.wg.Go(func() {
		h.start(ctx, t)
		h.mu.Lock()
		delete(h.starting, t.ID)
		h.mu.Unlock()
	})
}

// start sends task.start and reports what the runner answered. A start the runner refused or failed
// is a part that did not start. One cut short by the link is left as it is: the task stays starting
// and the next tick sends the same fence again, which the runner joins.
func (h *holding) start(ctx context.Context, t domain.Task) {
	err := call(ctx, h.link, wire.KindTaskStart, startOf(t, h.c.cfg.Access(t)), nil)
	var we *wire.Error
	switch {
	case err == nil:
		h.report(ctx, t.ID, task.Report{Kind: task.PartsUp, Run: t.Run})
	case errors.As(err, &we) && we.Code != wire.CodeCancelled:
		h.report(ctx, t.ID, task.Report{Kind: task.PartFailed, Run: t.Run, Text: we.Msg})
	default:
		h.log.Info("start cut short", "task", t.ID, "err", err)
	}
}

func fenceOf(t domain.Task) wire.Fence { return wire.Fence{Task: string(t.ID), Run: t.Run, Gen: t.Gen} }

func startOf(t domain.Task, access wire.ProxyAccess) wire.TaskStart {
	s := wire.TaskStart{
		Fence: fenceOf(t), Repo: wire.Repo{Host: string(t.Repo.Host), Path: string(t.Repo.Path)},
		Commit: string(t.Commit), Prompt: t.Prompt, ReviewPrompt: t.ReviewPrompt, Proxies: access,
	}
	if t.Spec != nil {
		s.Spec = &wire.Spec{Name: string(t.Spec.Name), File: t.Spec.File}
	}
	if t.Event != nil {
		s.Event = &wire.Event{Source: string(t.Event.Source), Group: string(t.Event.Group), Rev: string(t.Event.Rev), Context: t.Event.Context}
	}
	return s
}

// report records a runner report. One the state machine finds stale or already past is expected
// when a line repeats or arrives late, so only other failures are logged.
func (h *holding) report(ctx context.Context, id domain.TaskID, r task.Report) {
	r.Runner = h.id
	_, err := h.c.cfg.Tasks.Report(ctx, id, r)
	if err != nil && !errors.Is(err, task.ErrStaleReport) && !errors.Is(err, domain.ErrRefused) && !errors.Is(err, domain.ErrNotFound) {
		h.log.Warn("report", "task", id, "err", err)
	}
}
