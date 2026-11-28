package inbound

import (
	"context"
	"fmt"
	"log/slog"
	"slices"
	"strings"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// Deps are the parts the service calls. Stop tells the runners of the tasks a new event superseded, once the
// transaction that made the new tasks has committed. Crons reads schedules, and Branches the default branch a
// schedule's or a Run's event names, which is left out of the event when Branches is nil or cannot read it.
type Deps struct {
	Store    Store
	Numbers  Numbers
	Tasks    Starter
	Specs    Specs
	Allowed  Allowed
	Link     Linker
	Crons    Crons
	Branches Branches
	Clock    task.Clock
	Config   task.Config
	Stop     func(ctx context.Context, effects []task.Effect)
	Log      *slog.Logger
}

type Service struct{ d Deps }

func New(d Deps) *Service {
	if d.Stop == nil {
		d.Stop = func(context.Context, []task.Effect) {}
	}
	if d.Log == nil {
		d.Log = slog.Default()
	}
	return &Service{d: d}
}

// Receive stores a delivery and starts a task for each task spec that hears it. A delivery whose id was stored
// before starts nothing and reports the tasks the first one started. A repository that Settings does not list
// starts nothing, and a spec that cannot fill its prompt from the event is passed over, with the reason in the
// delivery's outcome.
func (s *Service) Receive(ctx context.Context, in domain.Inbound) (domain.InboundResult, error) {
	return s.receiveFor(ctx, in, s.matching)
}

// matcher lists the specs that hear a delivery, or says why none does.
type matcher func(ctx context.Context, in domain.Inbound) ([]domain.SpecFile, string, error)

func (s *Service) receiveFor(ctx context.Context, in domain.Inbound, match matcher) (domain.InboundResult, error) {
	var res domain.InboundResult
	var effects []task.Effect
	err := s.d.Store.InTx(ctx, func(tx Tx) error {
		var err error
		res, effects, err = s.receive(ctx, tx, in, match)
		return err
	})
	if err != nil {
		return domain.InboundResult{}, fmt.Errorf("receive delivery %s: %w", in.DeliveryID, err)
	}
	s.d.Stop(context.WithoutCancel(ctx), effects)
	return res, nil
}

func (s *Service) receive(ctx context.Context, tx Tx, in domain.Inbound, match matcher) (domain.InboundResult, []task.Effect, error) {
	first, err := tx.RecordInbound(ctx, s.stored(in))
	if err != nil {
		return domain.InboundResult{}, nil, fmt.Errorf("store the delivery: %w", err)
	}
	if !first {
		res, err := s.seen(ctx, tx, in)
		return res, nil, err
	}
	matched, outcome, err := match(ctx, in)
	if err != nil {
		return domain.InboundResult{}, nil, err
	}
	res, effects := domain.InboundResult{Outcome: outcome}, []task.Effect(nil)
	if len(matched) > 0 {
		if res, effects, err = s.start(ctx, tx, in, matched); err != nil {
			return res, nil, err
		}
	}
	if err := tx.FinishInbound(ctx, in.Source, in.DeliveryID, res.Outcome); err != nil {
		return res, nil, fmt.Errorf("finish the delivery: %w", err)
	}
	return res, effects, nil
}

func (s *Service) stored(in domain.Inbound) domain.InboundEvent {
	return domain.InboundEvent{Source: in.Source, DeliveryID: in.DeliveryID, Event: in.Event, Action: in.Action, ReceivedAt: s.d.Clock.Now(), Outcome: "received"}
}

func (s *Service) seen(ctx context.Context, tx Tx, in domain.Inbound) (domain.InboundResult, error) {
	ev, ids, err := tx.Inbound(ctx, in.Source, in.DeliveryID)
	if err != nil {
		return domain.InboundResult{}, fmt.Errorf("read the stored delivery: %w", err)
	}
	return domain.InboundResult{Duplicate: true, Tasks: ids, Outcome: ev.Outcome}, nil
}

// matching lists the specs that hear the delivery, or says why none does.
func (s *Service) matching(ctx context.Context, in domain.Inbound) ([]domain.SpecFile, string, error) {
	if in.Occurrence == nil {
		return nil, in.Ignored, nil
	}
	allowed, err := s.d.Allowed.Allowed(ctx)
	if err != nil {
		return nil, "", fmt.Errorf("allowed repositories: %w", err)
	}
	if !slices.Contains(allowed, in.Occurrence.Incoming.Repo) {
		return nil, "ignored: " + in.Occurrence.Incoming.Repo.String() + " is not under repositories in Settings", nil
	}
	files, err := s.d.Specs.TaskSpecs(ctx)
	if err != nil {
		return nil, "", fmt.Errorf("task specs: %w", err)
	}
	var out []domain.SpecFile
	for _, f := range files {
		if f.Valid() && len(domain.RepositoriesNotAllowed(f.Spec, domain.Settings{Repositories: allowed})) == 0 && f.Spec.Applies(in.Occurrence.Incoming) {
			out = append(out, f)
		}
	}
	if len(out) == 0 {
		return nil, "ignored: no task spec hears it", nil
	}
	return out, "", nil
}

func (s *Service) start(ctx context.Context, tx Tx, in domain.Inbound, matched []domain.SpecFile) (domain.InboundResult, []task.Effect, error) {
	tasks := task.New(tx, s.d.Clock, s.d.Config)
	var res domain.InboundResult
	var effects []task.Effect
	var passed []string
	for _, f := range matched {
		prompts, err := f.Spec.Fill(in.Occurrence.Incoming.Payload, nil)
		if err != nil {
			passed = append(passed, fmt.Sprintf("%s: %v", f.Spec.Name, err))
			continue
		}
		id, err := tx.NextTaskID(ctx)
		if err != nil {
			return res, nil, fmt.Errorf("task number: %w", err)
		}
		out, err := tasks.StartByEvent(ctx, eventInput(id, in.Occurrence, f, prompts, ""))
		if err != nil {
			return res, nil, fmt.Errorf("start %s: %w", f.Spec.Name, err)
		}
		if err := tx.LinkInbound(ctx, in.Source, in.DeliveryID, out.Task.ID); err != nil {
			return res, nil, fmt.Errorf("start %s: %w", f.Spec.Name, err)
		}
		res.Tasks = append(res.Tasks, out.Task.ID)
		effects = append(effects, out.Effects...)
	}
	res.Outcome = fmt.Sprintf("started %d of %d tasks", len(res.Tasks), len(matched))
	if len(passed) > 0 {
		res.Outcome += ", passed over: " + strings.Join(passed, "; ")
	}
	return res, effects, nil
}

func eventInput(id domain.TaskID, occ *domain.Occurrence, f domain.SpecFile, p domain.Prompts, by domain.Person) task.EventInput {
	return task.EventInput{
		ID: id, Repo: occ.Incoming.Repo, Spec: domain.Spec{Name: f.Spec.Name, File: f.File}, HomeCommit: f.Commit, Event: occ.Event, Ref: occ.Ref, Commit: occ.Commit,
		Prompt: p.Prompt, ReviewPrompt: p.ReviewPrompt, By: by, TokenOwner: by, CancelInProgress: f.Spec.Concurrency.CancelInProgress,
	}
}
