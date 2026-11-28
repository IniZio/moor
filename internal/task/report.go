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

var reports = map[ReportKind]func(env, domain.Task, Report) (decision, error){
	SlotTaken:     env.slotTaken,
	PartsUp:       env.partsUp,
	PartFailed:    env.partFailed,
	HarnessExited: env.harnessExited,
	BoxRecreated:  env.boxRecreated,
}

// Report records a runner report. A repeat of one already recorded changes nothing.
func (s *Service) Report(ctx context.Context, id domain.TaskID, r Report) (Outcome, error) {
	handle, ok := reports[r.Kind]
	if !ok || (r.Kind == SlotTaken && r.Runner == "") {
		return Outcome{}, fmt.Errorf("%w: report kind %d with runner %q", ErrBadInput, r.Kind, r.Runner)
	}
	return s.apply(ctx, id, func(_ context.Context, e env, t domain.Task) (decision, error) {
		if r.Kind != SlotTaken && r.Run != t.Run {
			return decision{}, fmt.Errorf("task %s: %w: run %d, now %d", id, ErrStaleReport, r.Run, t.Run)
		}
		return handle(e, t, r)
	})
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

// harnessExited is settled in every state past running, since a session ends normally after a publish.
func (e env) harnessExited(t domain.Task, _ Report) (decision, error) {
	return e.run(t, op{
		signal:  domain.SessionEnded,
		settled: []domain.State{domain.Delivering, domain.Done, domain.Failed, domain.Superseded, domain.Cancelled},
		reason:  "session ended with nothing published",
	})
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
