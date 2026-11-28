package task

import (
	"context"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
)

// ReportKind is what a runner tells the portal about a task's parts.
type ReportKind uint8

const (
	// SlotTaken says a host took the task. Runner names the host. It starts a new run.
	SlotTaken ReportKind = iota + 1
	// PartsUp says the desk, the Box and the proxy are all up. Text lists them.
	PartsUp
	PartFailed
	HarnessExited
	BoxRecreated
	// Noted is a step the runner took, such as the desk coming up. Text is the note, as domain.NoteText makes it.
	Noted
)

// Report is one runner message. Run fences it: a report for an older run is refused, not applied.
type Report struct {
	Kind       ReportKind
	Run        int64
	Gen        int64
	Runner     domain.RunnerID
	Text       string
	HomeCommit string
}

// reports decide on the task alone. storeReports also read what the store holds of it.
var (
	reports = map[ReportKind]func(env, domain.Task, Report) (decision, error){
		SlotTaken:    env.slotTaken,
		PartsUp:      env.partsUp,
		PartFailed:   env.partFailed,
		BoxRecreated: env.boxRecreated,
	}
	storeReports = map[ReportKind]func(*Service, context.Context, env, domain.Task, Report) (decision, error){
		HarnessExited: (*Service).harnessExited,
		Noted:         (*Service).noted,
	}
)

// Report records a runner report. A repeat of one already recorded changes nothing.
func (s *Service) Report(ctx context.Context, id domain.TaskID, r Report) (Outcome, error) {
	if !r.valid() {
		return Outcome{}, fmt.Errorf("%w: report kind %d with runner %q and text %q", ErrBadInput, r.Kind, r.Runner, r.Text)
	}
	return s.apply(ctx, id, func(ctx context.Context, e env, t domain.Task) (decision, error) {
		if r.Kind != SlotTaken && r.Run != t.Run {
			return decision{}, fmt.Errorf("task %s: %w: run %d, now %d", id, ErrStaleReport, r.Run, t.Run)
		}
		if handle, ok := storeReports[r.Kind]; ok {
			return handle(s, ctx, e, t, r)
		}
		return reports[r.Kind](e, t, r)
	})
}

func (r Report) valid() bool {
	_, plain := reports[r.Kind]
	_, reads := storeReports[r.Kind]
	switch {
	case !plain && !reads:
		return false
	case r.Kind == SlotTaken:
		return r.Runner != ""
	case r.Kind == Noted:
		return r.Text != ""
	}
	return true
}

func (e env) slotTaken(t domain.Task, r Report) (decision, error) {
	if t.State != domain.Waiting && t.Runner == r.Runner {
		return decision{task: t}, nil
	}
	d, err := e.run(t, op{signal: domain.SlotFree, reason: "slot taken on " + string(r.Runner)})
	d.task.Runner = r.Runner
	d.task.Run = t.Run + 1
	return d, err
}

func (e env) partsUp(t domain.Task, r Report) (decision, error) {
	return e.run(t, op{
		signal: domain.PartsUp, settled: []domain.State{domain.Running, domain.Delivering, domain.Done},
		reason: orText(r.Text, "desk, box and proxy up"),
	})
}

func (e env) partFailed(t domain.Task, r Report) (decision, error) {
	return e.run(t, op{signal: domain.PartFailed, settled: []domain.State{domain.Failed}, reason: orText(r.Text, "a part did not start")})
}

// harnessExited is settled in every state past running, since a session ends normally after a publish. A session
// that ends while the first version waits for its review accepts that version, as the publish after the review would
// have: the result is there and nothing more is coming.
func (s *Service) harnessExited(ctx context.Context, e env, t domain.Task, _ Report) (decision, error) {
	if t.State == domain.Running && t.ReviewPrompt != "" {
		versions, err := s.store.Versions(ctx, t.ID)
		if err != nil {
			return decision{}, fmt.Errorf("load versions of %s: %w", t.ID, err)
		}
		if len(versions) == 1 {
			return e.accept(t, versions[0], "session ended after the first version")
		}
	}
	return e.run(t, op{
		signal:  domain.SessionEnded,
		settled: []domain.State{domain.Delivering, domain.Done, domain.Failed, domain.Superseded, domain.Cancelled},
		reason:  "session ended with nothing published",
	})
}

// accept moves a task that holds version v as if the publish that made it had been accepted.
func (e env) accept(t domain.Task, v domain.ResultVersion, reason string) (decision, error) {
	next, rows, err := e.move(t, domain.Published, reason)
	if err != nil {
		return decision{}, err
	}
	return decision{task: next, rows: rows, effects: e.owed(next, v, v.N)}, nil
}

// noted records one runner note, once. The runner sends its notes again when its watch reopens, so a note the
// task already has is a repeat.
func (s *Service) noted(ctx context.Context, _ env, t domain.Task, r Report) (decision, error) {
	if !t.State.Live() {
		return decision{}, fmt.Errorf("task %s: %w: a note needs a task that lives, not one that is %s", t.ID, domain.ErrRefused, t.State)
	}
	have, err := s.store.HasNote(ctx, t.ID, r.Text)
	if err != nil {
		return decision{}, fmt.Errorf("look for the note of %s: %w", t.ID, err)
	}
	if have {
		return decision{task: t}, nil
	}
	return decision{task: t, rows: []row{{domain.KindNote, domain.NoteData{Text: r.Text}}}}, nil
}

func (e env) boxRecreated(t domain.Task, r Report) (decision, error) {
	if _, err := domain.Next(e.flow(t), t.State, domain.BoxRecreated); err != nil {
		return decision{}, fmt.Errorf("task %s: %w", t.ID, err)
	}
	if r.Gen <= t.Gen {
		return decision{task: t}, nil
	}
	t.Gen = r.Gen
	return decision{task: t, rows: []row{{domain.KindBox, domain.BoxData{Gen: r.Gen, HomeCommit: r.HomeCommit}}}}, nil
}

func orText(text, fallback string) string {
	if text == "" {
		return fallback
	}
	return text
}
