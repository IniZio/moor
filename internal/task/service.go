package task

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

var (
	ErrBadInput    = errors.New("bad input")
	ErrIDTaken     = errors.New("task id taken by different inputs")
	ErrStaleReport = errors.New("report for an older run")
)

const defaultAttempts = 5

type Service struct {
	store Store
	clock Clock
	cfg   Config
}

func New(store Store, clock Clock, cfg Config) *Service {
	if cfg.Attempts <= 0 {
		cfg.Attempts = defaultAttempts
	}
	return &Service{store: store, clock: clock, cfg: cfg}
}

// env is one attempt's view of time and limits. A decision is a pure function of a task and an env.
type env struct {
	now time.Time
	cfg Config
}

func (s *Service) env() env { return env{now: s.clock.Now(), cfg: s.cfg} }

type row struct {
	kind domain.EventKind
	data any
}

// decision is the new task, the rows to record (none means no change) and the effects to report.
type decision struct {
	task    domain.Task
	rows    []row
	result  *domain.ResultVersion
	effects []Effect
}

type decide func(ctx context.Context, e env, t domain.Task) (decision, error)

// apply loads the task, decides, and saves, reading again when another writer got in first.
func (s *Service) apply(ctx context.Context, id domain.TaskID, fn decide) (Outcome, error) {
	var out Outcome
	err := s.retry(fmt.Sprintf("task %s", id), func() error {
		var err error
		out, err = s.attempt(ctx, id, fn)
		return err
	}, domain.ErrStaleVersion)
	return out, err
}

func (s *Service) attempt(ctx context.Context, id domain.TaskID, fn decide) (Outcome, error) {
	t, err := s.store.Load(ctx, id)
	if err != nil {
		return Outcome{}, fmt.Errorf("load task %s: %w", id, err)
	}
	e := s.env()
	d, err := fn(ctx, e, t)
	if err != nil {
		return Outcome{}, err
	}
	if len(d.rows) == 0 {
		return Outcome{Task: t, Effects: d.effects}, nil
	}
	return s.commit(ctx, e, d, false)
}

func (s *Service) commit(ctx context.Context, e env, d decision, create bool) (Outcome, error) {
	events, err := eventRows(d.task, d.rows, e.now)
	if err != nil {
		return Outcome{}, err
	}
	d.task.LastSeq += int64(len(events))
	saved, err := s.store.Save(ctx, domain.TaskWrite{Task: d.task, Create: create, Events: events, Result: d.result})
	if err != nil {
		return Outcome{}, fmt.Errorf("save task %s: %w", d.task.ID, err)
	}
	return Outcome{Task: saved, Events: events, Effects: d.effects}, nil
}

func eventRows(t domain.Task, rows []row, now time.Time) ([]domain.EventRow, error) {
	events := make([]domain.EventRow, 0, len(rows))
	for i, r := range rows {
		data, err := json.Marshal(r.data)
		if err != nil {
			return nil, fmt.Errorf("encode %s event: %w", r.kind, err)
		}
		events = append(events, domain.EventRow{Task: t.ID, Seq: t.LastSeq + int64(i) + 1, At: now, Kind: r.kind, Data: data})
	}
	return events, nil
}

// retry runs fn again while it fails with one of again, up to the configured attempts.
func (s *Service) retry(what string, fn func() error, again ...error) error {
	var err error
	for range s.cfg.Attempts {
		err = fn()
		if err == nil || !slices.ContainsFunc(again, func(target error) bool { return errors.Is(err, target) }) {
			return err
		}
	}
	return fmt.Errorf("%s: gave up after %d attempts: %w", what, s.cfg.Attempts, err)
}
