package store

import (
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

var taskColumns = []string{
	"id", "repo_host", "repo_path", "prompt", "review_prompt", "started_by",
	"spec_name", "spec_file", "event_source", "event_group", "event_rev", "event_context",
	"state", "version", "commit", "ref", "machine_id", "run", "gen", "last_seq",
	"deadline", "keep_until", "failure", "token_owner", "created_at", "ended_at",
}

var (
	selectTask = "SELECT " + strings.Join(taskColumns, ", ") + " FROM tasks "
	insertTask = "INSERT INTO tasks (" + strings.Join(taskColumns, ", ") + ") VALUES (" + placeholders(len(taskColumns)) + ")"
	updateTask = "UPDATE tasks SET " + assignments(taskColumns[1:]) + " WHERE id = $1"
)

func placeholders(n int) string {
	marks := make([]string, n)
	for i := range marks {
		marks[i] = "$" + strconv.Itoa(i+1)
	}
	return strings.Join(marks, ", ")
}

// assignments numbers the columns from $2, leaving $1 for the id.
func assignments(cols []string) string {
	parts := make([]string, len(cols))
	for i, c := range cols {
		parts[i] = c + " = $" + strconv.Itoa(i+2)
	}
	return strings.Join(parts, ", ")
}

// taskArgs lists t's values in taskColumns order. Zero times and empty bytes are NULL.
func taskArgs(t domain.Task, version int64) ([]any, error) {
	state, err := t.State.MarshalText()
	if err != nil {
		return nil, fmt.Errorf("task %s: %w", t.ID, err)
	}
	specName, specFile := specArgs(t.Spec)
	source, group, rev, context := eventArgs(t.Event)
	return []any{
		string(t.ID), string(t.Repo.Host), string(t.Repo.Path), t.Prompt, t.ReviewPrompt, t.StartedBy,
		specName, specFile, source, group, rev, context,
		string(state), version, string(t.Commit), t.Ref, string(t.Runner), t.Run, t.Gen, t.LastSeq,
		timeOrNil(t.Deadline), timeOrNil(t.KeepUntil), t.Failure, string(t.TokenOwner), t.CreatedAt, timeOrNil(t.EndedAt),
	}, nil
}

func specArgs(sp *domain.Spec) (name, file *string) {
	if sp == nil {
		return nil, nil
	}
	n, f := string(sp.Name), sp.File
	return &n, &f
}

func eventArgs(ev *domain.Event) (source, group, rev *string, context []byte) {
	if ev == nil {
		return nil, nil, nil, nil
	}
	s, g, r := string(ev.Source), string(ev.Group), string(ev.Rev)
	return &s, &g, &r, nilIfEmpty(ev.Context)
}

func nilIfEmpty(b []byte) []byte {
	if len(b) == 0 {
		return nil
	}
	return b
}

func timeOrNil(t time.Time) any {
	if t.IsZero() {
		return nil
	}
	return t
}

func scanTask(row pgx.Row) (domain.Task, error) { return scanTaskWith(row) }

// scanTaskWith scans the task columns, then the extra destinations for the columns a query adds after them.
func scanTaskWith(row pgx.Row, extra ...any) (domain.Task, error) {
	var (
		t                            domain.Task
		specName, specFile           *string
		source, group, rev           *string
		context                      []byte
		state                        string
		deadline, keepUntil, endedAt *time.Time
	)
	dest := []any{
		&t.ID, &t.Repo.Host, &t.Repo.Path, &t.Prompt, &t.ReviewPrompt, &t.StartedBy, &specName, &specFile, &source, &group, &rev, &context,
		&state, &t.Version, &t.Commit, &t.Ref, &t.Runner, &t.Run, &t.Gen, &t.LastSeq,
		&deadline, &keepUntil, &t.Failure, &t.TokenOwner, &t.CreatedAt, &endedAt,
	}
	err := row.Scan(append(dest, extra...)...)
	if err != nil {
		return domain.Task{}, fmt.Errorf("scan task: %w", err)
	}
	if t.State, err = domain.ParseState(state); err != nil {
		return domain.Task{}, fmt.Errorf("task %s: %w", t.ID, err)
	}
	if specName != nil {
		t.Spec = &domain.Spec{Name: domain.SpecName(*specName), File: *specFile}
	}
	if source != nil {
		t.Event = &domain.Event{Source: domain.EventSource(*source), Group: domain.EventGroup(*group), Rev: domain.Revision(*rev), Context: context}
	}
	t.CreatedAt = t.CreatedAt.UTC()
	t.Deadline, t.KeepUntil, t.EndedAt = utc(deadline), utc(keepUntil), utc(endedAt)
	return t, nil
}

func utc(t *time.Time) time.Time {
	if t == nil {
		return time.Time{}
	}
	return t.UTC()
}
