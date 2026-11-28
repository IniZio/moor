package task

import (
	"context"
	"errors"
	"fmt"
	"maps"

	"github.com/IniZio/moor/internal/domain"
)

// StartInput starts a task for a person from a plain prompt. ID comes from the caller, so a
// retried request names the same task. A task with no review prompt ends on its first publish.
type StartInput struct {
	ID           domain.TaskID
	Project      domain.Project
	Prompt       string
	ReviewPrompt string
	Refs         map[domain.Dir]string
	By           domain.Person
}

// StartByPerson makes a waiting task. The same ID with the same inputs returns that task.
func (s *Service) StartByPerson(ctx context.Context, in StartInput) (Outcome, error) {
	t, err := personTask(in)
	if err != nil {
		return Outcome{}, err
	}
	return s.create(ctx, t, []row{createdRow("started by " + string(in.By))})
}

func personTask(in StartInput) (domain.Task, error) {
	if in.ID == "" || in.By == "" || in.Project.Name() == "" {
		return domain.Task{}, fmt.Errorf("%w: a task needs an id, a person and a project", ErrBadInput)
	}
	if in.Prompt == "" {
		return domain.Task{}, fmt.Errorf("%w: no prompt", ErrBadInput)
	}
	if err := checkRefs(in.Project, in.Refs); err != nil {
		return domain.Task{}, err
	}
	return domain.Task{
		ID: in.ID, Project: in.Project.Name(), Prompt: in.Prompt, ReviewPrompt: in.ReviewPrompt,
		StartedBy: string(in.By), Refs: maps.Clone(in.Refs), State: domain.Waiting,
	}, nil
}

func checkRefs(p domain.Project, refs map[domain.Dir]string) error {
	dirs := make(map[domain.Dir]bool)
	for _, e := range p.Entries() {
		dirs[e.Dir] = true
	}
	for dir, ref := range refs {
		if !dirs[dir] || ref == "" {
			return fmt.Errorf("%w: no entry %q or an empty ref in project %s", ErrBadInput, dir, p.Name())
		}
	}
	return nil
}

// Retry starts a new task with a failed plain-prompt task's inputs and records the link on it.
// An event task has no retry: its key stays taken, and a newer revision starts afresh.
func (s *Service) Retry(ctx context.Context, newID, oldID domain.TaskID) (Outcome, error) {
	old, err := s.store.Load(ctx, oldID)
	if err != nil {
		return Outcome{}, fmt.Errorf("load task %s: %w", oldID, err)
	}
	if old.State != domain.Failed || old.Event != nil || newID == "" {
		return Outcome{}, fmt.Errorf("task %s: %w: retry needs a failed plain-prompt task and a new id", oldID, domain.ErrRefused)
	}
	t := domain.Task{
		ID: newID, Project: old.Project, Prompt: old.Prompt, ReviewPrompt: old.ReviewPrompt,
		StartedBy: old.StartedBy, Refs: maps.Clone(old.Refs), State: domain.Waiting,
	}
	rows := []row{createdRow("retry of " + string(oldID)), {domain.KindNote, domain.NoteData{Text: "retry of " + string(oldID)}}}
	return s.create(ctx, t, rows)
}

func createdRow(reason string) row {
	return row{domain.KindState, domain.StateData{To: domain.Waiting.String(), Reason: reason}}
}

// create stores a new task unless its id, or its event key, already names one.
func (s *Service) create(ctx context.Context, t domain.Task, rows []row) (Outcome, error) {
	var out Outcome
	err := s.retry(fmt.Sprintf("start %s", t.ID), func() error {
		var err error
		out, err = s.createOnce(ctx, t, rows)
		return err
	}, domain.ErrExists)
	return out, err
}

func (s *Service) createOnce(ctx context.Context, t domain.Task, rows []row) (Outcome, error) {
	existing, found, err := s.find(ctx, t)
	if err != nil || found {
		return Outcome{Task: existing}, err
	}
	e := s.env()
	t.CreatedAt = e.now
	return s.commit(ctx, e, decision{task: t, rows: rows}, true)
}

func (s *Service) find(ctx context.Context, t domain.Task) (domain.Task, bool, error) {
	if key, ok := t.Key(); ok {
		got, err := s.store.ByKey(ctx, key)
		if err == nil {
			return got, true, nil
		}
		if !errors.Is(err, domain.ErrNotFound) {
			return domain.Task{}, false, fmt.Errorf("find task by key: %w", err)
		}
	}
	got, err := s.store.Load(ctx, t.ID)
	switch {
	case err == nil && sameInputs(got, t):
		return got, true, nil
	case err == nil:
		return domain.Task{}, false, fmt.Errorf("%w: %s", ErrIDTaken, t.ID)
	case errors.Is(err, domain.ErrNotFound):
		return domain.Task{}, false, nil
	}
	return domain.Task{}, false, fmt.Errorf("load task %s: %w", t.ID, err)
}
