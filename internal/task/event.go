package task

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// EventInput starts a task for a spec's event. ID is only used when the key is new. By names the person
// who ran the spec by hand. CancelInProgress is the spec's setting: only then does a newer event supersede.
type EventInput struct {
	ID               domain.TaskID
	Repo             domain.Repo
	Spec             domain.Spec
	Event            domain.Event
	Ref              string
	Commit           domain.Commit
	Prompt           string
	ReviewPrompt     string
	By               domain.Person
	TokenOwner       domain.Person
	CancelInProgress bool
}

// StartByEvent makes a waiting task, then supersedes the older live tasks of its group when the
// spec says so. The same key finds the existing task and finishes any supersede that a crash left undone.
func (s *Service) StartByEvent(ctx context.Context, in EventInput) (Outcome, error) {
	t, err := eventTask(in)
	if err != nil {
		return Outcome{}, err
	}
	var out Outcome
	err = s.retry(fmt.Sprintf("start %s", t.ID), func() error {
		var err error
		out, err = s.startEvented(ctx, t, in.CancelInProgress)
		return err
	}, domain.ErrExists)
	return out, err
}

func eventTask(in EventInput) (domain.Task, error) {
	ev, sp := in.Event, in.Spec
	if in.ID == "" || !in.Repo.Complete() || sp.Name == "" || sp.File == "" || ev.Source == "" || ev.Group == "" || ev.Rev == "" {
		return domain.Task{}, fmt.Errorf("%w: an event task needs an id, a repository, a spec name and file, a source, a group and a rev", ErrBadInput)
	}
	ev.Context = bytes.Clone(ev.Context)
	startedBy := domain.EventStarterPrefix + string(ev.Source)
	if in.By != "" {
		startedBy = string(in.By)
	}
	return domain.Task{
		ID: in.ID, Repo: in.Repo, Prompt: in.Prompt, ReviewPrompt: in.ReviewPrompt,
		StartedBy: startedBy, Spec: &sp, Event: &ev, TokenOwner: in.TokenOwner, State: domain.Waiting,
		Ref: in.Ref, Commit: in.Commit,
	}, nil
}

func (s *Service) startEvented(ctx context.Context, t domain.Task, cancelInProgress bool) (Outcome, error) {
	e := s.env()
	existing, found, err := s.find(ctx, t)
	if err != nil {
		return Outcome{}, err
	}
	out := Outcome{Task: existing}
	if !found {
		if t.CreatedAt, err = s.createdAfterLive(ctx, t, e.now); err != nil {
			return Outcome{}, err
		}
		if out, err = s.commit(ctx, e, decision{task: t, rows: []row{createdRow("started by event")}}, true); err != nil {
			return Outcome{}, err
		}
	}
	if !cancelInProgress {
		return out, nil
	}
	older, err := s.supersedeOlder(ctx, out.Task)
	out.merge(older)
	return out, err
}

// createdAfterLive keeps creation times in arrival order within a group, which is what "older" means.
func (s *Service) createdAfterLive(ctx context.Context, t domain.Task, now time.Time) (time.Time, error) {
	live, err := s.groupOf(ctx, t)
	if err != nil {
		return time.Time{}, err
	}
	for _, l := range live {
		if !now.After(l.CreatedAt) {
			now = l.CreatedAt.Add(time.Nanosecond)
		}
	}
	return now, nil
}

func (s *Service) groupOf(ctx context.Context, t domain.Task) ([]domain.Task, error) {
	live, err := s.store.LiveInGroup(ctx, t.Repo, t.Spec.Name, t.Event.Source, t.Event.Group)
	if err != nil {
		return nil, fmt.Errorf("list live tasks of %s: %w", t.Event.Group, err)
	}
	return live, nil
}

func (s *Service) supersedeOlder(ctx context.Context, t domain.Task) (Outcome, error) {
	live, err := s.groupOf(ctx, t)
	if err != nil {
		return Outcome{}, err
	}
	var out Outcome
	for _, old := range live {
		if old.ID == t.ID || !old.CreatedAt.Before(t.CreatedAt) {
			continue
		}
		o, err := s.apply(ctx, old.ID, supersede(t.Event.Rev))
		if errors.Is(err, domain.ErrRefused) {
			continue
		}
		if err != nil {
			return Outcome{}, err
		}
		out.merge(o)
	}
	return out, nil
}

func supersede(rev domain.Revision) decide {
	return func(_ context.Context, e env, t domain.Task) (decision, error) {
		return e.run(t, op{
			signal: domain.NewerRev, settled: []domain.State{domain.Superseded},
			reason: "newer rev " + string(rev), effects: stopJob,
		})
	}
}

func sameInputs(a, b domain.Task) bool {
	return a.Repo == b.Repo && a.Prompt == b.Prompt && a.ReviewPrompt == b.ReviewPrompt &&
		a.StartedBy == b.StartedBy && a.TokenOwner == b.TokenOwner && a.Ref == b.Ref && a.Commit == b.Commit &&
		sameSpec(a.Spec, b.Spec) && sameEvent(a.Event, b.Event)
}

func sameSpec(a, b *domain.Spec) bool {
	if a == nil || b == nil {
		return a == b
	}
	return *a == *b
}

func sameEvent(a, b *domain.Event) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.Source == b.Source && a.Group == b.Group && a.Rev == b.Rev && bytes.Equal(a.Context, b.Context)
}
