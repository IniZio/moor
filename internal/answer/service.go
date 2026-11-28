package answer

import (
	"context"
	"errors"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// Poster is how one source takes an answer: where it goes, what it says, and how to make it and change it.
// Find looks the made answer up again by what it says, for when its id was lost.
type Poster interface {
	Target(ev domain.Event) (domain.DeliveryTarget, bool)
	Body(d domain.Delivery) string
	Find(ctx context.Context, t domain.DeliveryTarget) (externalID string, found bool, err error)
	Post(ctx context.Context, t domain.DeliveryTarget, body string) (externalID string, err error)
	Edit(ctx context.Context, t domain.DeliveryTarget, externalID, body string) error
}

// Deliveries stores the delivery of a place. Locked runs fn with it while no other caller holds the place.
type Deliveries interface {
	Locked(ctx context.Context, kind, ref string, fn func(domain.Delivery) (domain.Delivery, error)) error
}

// Tasks is the task service's side of an answer.
type Tasks interface {
	Publish(ctx context.Context, id domain.TaskID, in task.PublishInput) (task.Outcome, error)
	Delivered(ctx context.Context, id domain.TaskID, in task.DeliveredInput) (task.Outcome, error)
	AnswerFailed(ctx context.Context, id domain.TaskID, version int, reason string) (task.Outcome, error)
}

type Deps struct {
	Tasks      Tasks
	Reads      task.Store
	Deliveries Deliveries
	Posters    map[domain.EventSource]Poster
	Clock      task.Clock
}

type Service struct{ d Deps }

func New(d Deps) *Service { return &Service{d: d} }

// Publish records a result version and, when that leaves the task owing a delivery, makes it. A failed delivery
// is recorded on the task and returned, and the version stays published.
func (s *Service) Publish(ctx context.Context, id domain.TaskID, in task.PublishInput) (task.Outcome, error) {
	out, err := s.d.Tasks.Publish(ctx, id, in)
	if err != nil {
		return out, fmt.Errorf("publish: %w", err)
	}
	for _, fx := range out.Effects {
		if fx.Kind != task.Deliver {
			continue
		}
		if err := s.deliver(ctx, fx); err != nil {
			return out, err
		}
	}
	return out, nil
}

var errNoPoster = errors.New("the event's source has nowhere to answer")

func (s *Service) deliver(ctx context.Context, fx task.Effect) error {
	err := s.post(ctx, fx)
	if err == nil {
		return nil
	}
	if _, recErr := s.d.Tasks.AnswerFailed(ctx, fx.Task, fx.Version, err.Error()); recErr != nil {
		err = errors.Join(err, fmt.Errorf("record the failure: %w", recErr))
	}
	return fmt.Errorf("answer task %s version %d: %w", fx.Task, fx.Version, err)
}

func (s *Service) post(ctx context.Context, fx task.Effect) error {
	t, err := s.d.Reads.Load(ctx, fx.Task)
	if err != nil {
		return fmt.Errorf("load task: %w", err)
	}
	versions, err := s.d.Reads.Versions(ctx, fx.Task)
	if err != nil || fx.Version < 1 || fx.Version > len(versions) {
		return fmt.Errorf("load version %d: %w", fx.Version, errors.Join(err, domain.ErrNotFound))
	}
	poster, target, ok := s.target(t)
	if !ok {
		return errNoPoster
	}
	section := domain.Section{Task: t.ID, Rev: t.Event.Rev, Version: fx.Version, Line: versions[fx.Version-1].Line, URL: versions[fx.Version-1].EntryURL}
	var externalID string
	err = s.d.Deliveries.Locked(ctx, target.Kind, target.Ref, func(d domain.Delivery) (domain.Delivery, error) {
		if d.Sections == nil {
			d.Sections = map[domain.SpecName]domain.Section{}
		}
		d.Sections[t.Spec.Name] = section
		d.UpdatedAt = s.d.Clock.Now()
		var err error
		d.ExternalID, err = s.write(ctx, poster, target, d)
		externalID = d.ExternalID
		return d, err
	})
	if err != nil {
		return fmt.Errorf("write the delivery: %w", err)
	}
	_, err = s.d.Tasks.Delivered(ctx, t.ID, task.DeliveredInput{Target: target.Label, ExternalID: externalID, Version: fx.Version})
	if err != nil {
		return fmt.Errorf("mark delivered: %w", err)
	}
	return nil
}

func (s *Service) target(t domain.Task) (Poster, domain.DeliveryTarget, bool) {
	if t.Event == nil || t.Spec == nil {
		return nil, domain.DeliveryTarget{}, false
	}
	p, ok := s.d.Posters[t.Event.Source]
	if !ok {
		return nil, domain.DeliveryTarget{}, false
	}
	target, ok := p.Target(*t.Event)
	return p, target, ok
}

// write creates the answer, or edits it when it exists, and gives its id. An answer the store lost the id of is
// found again by what it says, rather than made twice.
func (s *Service) write(ctx context.Context, p Poster, target domain.DeliveryTarget, d domain.Delivery) (string, error) {
	body := p.Body(d)
	if d.ExternalID == "" {
		id, found, err := p.Find(ctx, target)
		if err != nil {
			return "", fmt.Errorf("look for an earlier answer: %w", err)
		}
		d.ExternalID = id
		if !found {
			made, err := p.Post(ctx, target, body)
			if err != nil {
				return "", fmt.Errorf("post the answer on %s: %w", target.Label, err)
			}
			return made, nil
		}
	}
	if err := p.Edit(ctx, target, d.ExternalID, body); err != nil {
		return "", fmt.Errorf("edit the answer on %s: %w", target.Label, err)
	}
	return d.ExternalID, nil
}
