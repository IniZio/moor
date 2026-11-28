package task

import (
	"context"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
)

// simple runs a decision that needs only the task.
func (s *Service) simple(ctx context.Context, id domain.TaskID, fn func(env, domain.Task) (decision, error)) (Outcome, error) {
	return s.apply(ctx, id, func(_ context.Context, e env, t domain.Task) (decision, error) { return fn(e, t) })
}

// End is a person ending a task a person started. It ends as done, with or without a page.
func (s *Service) End(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.simple(ctx, id, func(e env, t domain.Task) (decision, error) {
		return e.run(t, op{signal: domain.PersonEnded, settled: []domain.State{domain.Done}, reason: "ended by person"})
	})
}

// Cancel ends a task that has not finished. It tells the caller which job to stop.
func (s *Service) Cancel(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.simple(ctx, id, func(e env, t domain.Task) (decision, error) {
		return e.run(t, op{signal: domain.Cancel, settled: []domain.State{domain.Cancelled}, reason: "cancelled", effects: stopJob})
	})
}

// TimeLimit fails a running task whose deadline has passed. Before the deadline it changes nothing.
func (s *Service) TimeLimit(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.simple(ctx, id, func(e env, t domain.Task) (decision, error) {
		if t.State == domain.Running && (t.Deadline.IsZero() || e.now.Before(t.Deadline)) {
			return decision{task: t}, nil
		}
		return e.run(t, op{signal: domain.TimeLimit, settled: []domain.State{domain.Failed}, reason: "time limit reached"})
	})
}

// MachineRevoked fails a task whose machine was revoked while it started or ran.
func (s *Service) MachineRevoked(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.simple(ctx, id, func(e env, t domain.Task) (decision, error) {
		return e.run(t, op{signal: domain.RunnerRevoked, settled: []domain.State{domain.Failed}, reason: "machine revoked"})
	})
}

// Released records, once, that the keep window of a done or failed task has ended, so its containers may be
// removed. It is refused before the window ends, for a task whose window has no end, and for a task that is not
// done or failed, such as one a teammate reopened inside its window.
func (s *Service) Released(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.apply(ctx, id, func(ctx context.Context, e env, t domain.Task) (decision, error) {
		ended := t.State == domain.Done || t.State == domain.Failed
		if !ended || t.KeepUntil.IsZero() || e.now.Before(t.KeepUntil) {
			return decision{}, fmt.Errorf("task %s: %w: no keep window has ended on a %s task", id, domain.ErrRefused, t.State)
		}
		text := domain.NoteText(domain.PhaseReleased, "keep window ended")
		have, err := s.store.HasNote(ctx, id, text)
		if err != nil || have {
			return decision{task: t}, wrapIf("look for the release note of "+string(id), err)
		}
		return decision{task: t, rows: []row{{domain.KindNote, domain.NoteData{Text: text}}}}, nil
	})
}

func wrapIf(what string, err error) error {
	if err == nil {
		return nil
	}
	return fmt.Errorf("%s: %w", what, err)
}

// Steer is input into the session. On a done task inside its keep window it reopens the task.
func (s *Service) Steer(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.simple(ctx, id, func(e env, t domain.Task) (decision, error) {
		if t.State == domain.Done && !t.KeepUntil.IsZero() && !e.now.Before(t.KeepUntil) {
			return decision{}, fmt.Errorf("task %s: %w: keep window ended", id, domain.ErrRefused)
		}
		return e.run(t, op{signal: domain.Steered, settled: []domain.State{domain.Running}, reason: "teammate typed"})
	})
}
