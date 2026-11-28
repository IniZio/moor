package task

import (
	"fmt"
	"slices"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// op names one domain signal and the states where a repeat of it is already done.
type op struct {
	signal  domain.Signal
	settled []domain.State
	reason  string
	effects func(domain.Task) []Effect
}

// run moves the task by the op, or does nothing when the op already took effect. It decides on the
// task's state alone, so any retry of any earlier attempt lands on the same end state.
func (e env) run(t domain.Task, o op) (decision, error) {
	next, rows, err := e.move(t, o.signal, o.reason)
	if err != nil {
		if slices.Contains(o.settled, t.State) && domain.Handles(e.flow(t), o.signal) {
			return decision{task: t, effects: o.fx(t)}, nil
		}
		return decision{}, err
	}
	return decision{task: next, rows: rows, effects: o.fx(next)}, nil
}

func (o op) fx(t domain.Task) []Effect {
	if o.effects == nil {
		return nil
	}
	return o.effects(t)
}

func (e env) move(t domain.Task, sig domain.Signal, reason string) (domain.Task, []row, error) {
	to, err := domain.Next(e.flow(t), t.State, sig)
	if err != nil {
		return t, nil, fmt.Errorf("task %s: %w", t.ID, err)
	}
	from := t.State
	t.State = to
	e.stamp(&t, from, reason)
	rows := []row{{domain.KindState, domain.StateData{From: from.String(), To: to.String(), Reason: reason}}}
	if to == domain.Failed {
		rows = append(rows, row{domain.KindError, domain.ErrorData{Text: reason}})
	}
	return t, rows, nil
}

// stamp sets the times that depend on the state a task enters.
func (e env) stamp(t *domain.Task, from domain.State, reason string) {
	switch t.State {
	case domain.Done, domain.Failed:
		t.EndedAt = e.now
		t.KeepUntil = e.after(e.cfg.KeepWindow)
		if t.State == domain.Failed {
			t.Failure = reason
		}
	case domain.Superseded, domain.Cancelled:
		t.EndedAt = e.now
	case domain.Running:
		t.Deadline = e.after(e.cfg.TimeLimit)
		if from == domain.Done {
			t.EndedAt, t.KeepUntil = time.Time{}, time.Time{}
		}
	case domain.Waiting, domain.Starting, domain.Delivering:
	}
}

func (e env) after(d time.Duration) time.Time {
	if d <= 0 {
		return time.Time{}
	}
	return e.now.Add(d)
}

func stopJob(t domain.Task) []Effect {
	if t.Runner == "" {
		return nil
	}
	return []Effect{{Kind: StopJob, Task: t.ID, Runner: t.Runner, Run: t.Run, Gen: t.Gen}}
}
