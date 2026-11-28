package task

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"maps"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// TriggerInput starts a task for a recipe's trigger. ID is only used when the key is new.
type TriggerInput struct {
	ID         domain.TaskID
	Project    domain.Project
	Recipe     domain.Recipe
	Trigger    domain.Trigger
	TokenOwner domain.Person
}

// StartByTrigger makes a waiting task, then supersedes the older live tasks of its group. The same
// key finds the existing task and finishes any supersede that a crash left undone.
func (s *Service) StartByTrigger(ctx context.Context, in TriggerInput) (Outcome, error) {
	t, err := triggerTask(in)
	if err != nil {
		return Outcome{}, err
	}
	var out Outcome
	err = s.retry(fmt.Sprintf("start %s", t.ID), func() error {
		var err error
		out, err = s.startTriggered(ctx, t)
		return err
	}, domain.ErrExists)
	return out, err
}

func triggerTask(in TriggerInput) (domain.Task, error) {
	tr := in.Trigger
	if in.ID == "" || in.Project.Name() == "" || tr.Source == "" || tr.Group == "" || tr.Rev == "" {
		return domain.Task{}, fmt.Errorf("%w: a trigger task needs an id, a project, a source, a group and a rev", ErrBadInput)
	}
	if in.Recipe.Name == "" || in.Recipe.Source != tr.Source {
		return domain.Task{}, fmt.Errorf("%w: recipe %q is not started by source %q", ErrBadInput, in.Recipe.Name, tr.Source)
	}
	tr.Context = bytes.Clone(tr.Context)
	return domain.Task{
		ID: in.ID, Project: in.Project.Name(), Prompt: in.Recipe.Prompt, Kind: in.Recipe.Name,
		StartedBy: "trigger:" + string(tr.Source), Trigger: &tr, TokenOwner: in.TokenOwner, State: domain.Waiting,
	}, nil
}

func (s *Service) startTriggered(ctx context.Context, t domain.Task) (Outcome, error) {
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
		if out, err = s.commit(ctx, e, decision{task: t, rows: []row{createdRow("started by trigger")}}, true); err != nil {
			return Outcome{}, err
		}
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
	live, err := s.store.LiveInGroup(ctx, t.Project, t.Trigger.Source, t.Trigger.Group)
	if err != nil {
		return nil, fmt.Errorf("list live tasks of %s: %w", t.Trigger.Group, err)
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
		o, err := s.apply(ctx, old.ID, supersede(t.Trigger.Rev))
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
			event: domain.NewerRev, settled: []domain.State{domain.Superseded},
			reason: "newer rev " + string(rev), effects: stopJob,
		})
	}
}

func sameInputs(a, b domain.Task) bool {
	return a.Project == b.Project && a.Prompt == b.Prompt && a.Kind == b.Kind && a.StartedBy == b.StartedBy &&
		a.TokenOwner == b.TokenOwner && maps.Equal(a.Refs, b.Refs) && sameTrigger(a.Trigger, b.Trigger)
}

func sameTrigger(a, b *domain.Trigger) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.Source == b.Source && a.Group == b.Group && a.Rev == b.Rev && bytes.Equal(a.Context, b.Context)
}
