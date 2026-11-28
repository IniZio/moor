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
	"id", "project", "prompt", "review_prompt", "reply_source", "reply_ref", "started_by",
	"trigger_name", "trigger_source", "trigger_group", "trigger_rev", "trigger_context",
	"state", "version", "pins", "refs", "runner_id", "run", "gen", "last_seq",
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

// taskArgs lists t's values in taskColumns order. Zero times, empty maps and empty bytes are NULL.
func taskArgs(t domain.Task, version int64) ([]any, error) {
	state, err := t.State.MarshalText()
	if err != nil {
		return nil, fmt.Errorf("task %s: %w", t.ID, err)
	}
	var name, source, group, rev *string
	var context []byte
	if tr := t.Trigger; tr != nil {
		n, s, g, r := string(tr.Name), string(tr.Source), string(tr.Group), string(tr.Rev)
		name, source, group, rev, context = &n, &s, &g, &r, nilIfEmpty(tr.Context)
	}
	return []any{
		string(t.ID), string(t.Project), t.Prompt, t.ReviewPrompt, t.Reply.Source, t.Reply.Ref, t.StartedBy,
		name, source, group, rev, context,
		string(state), version, mapOrNil(t.Pins), mapOrNil(t.Refs), string(t.Runner), t.Run, t.Gen, t.LastSeq,
		timeOrNil(t.Deadline), timeOrNil(t.KeepUntil), t.Failure, string(t.TokenOwner), t.CreatedAt, timeOrNil(t.EndedAt),
	}, nil
}

func nilIfEmpty(b []byte) []byte {
	if len(b) == 0 {
		return nil
	}
	return b
}

func mapOrNil[K ~string, V any](m map[K]V) any {
	if len(m) == 0 {
		return nil
	}
	return m
}

func timeOrNil(t time.Time) any {
	if t.IsZero() {
		return nil
	}
	return t
}

func scanTask(row pgx.Row) (domain.Task, error) {
	var (
		t                            domain.Task
		name, source, group, rev     *string
		context                      []byte
		state                        string
		deadline, keepUntil, endedAt *time.Time
	)
	err := row.Scan(&t.ID, &t.Project, &t.Prompt, &t.ReviewPrompt, &t.Reply.Source, &t.Reply.Ref, &t.StartedBy, &name, &source, &group, &rev, &context,
		&state, &t.Version, &t.Pins, &t.Refs, &t.Runner, &t.Run, &t.Gen, &t.LastSeq,
		&deadline, &keepUntil, &t.Failure, &t.TokenOwner, &t.CreatedAt, &endedAt)
	if err != nil {
		return domain.Task{}, fmt.Errorf("scan task: %w", err)
	}
	if t.State, err = domain.ParseState(state); err != nil {
		return domain.Task{}, fmt.Errorf("task %s: %w", t.ID, err)
	}
	if source != nil {
		t.Trigger = &domain.Trigger{Name: domain.TriggerName(*name), Source: domain.TriggerSource(*source), Group: domain.TriggerGroup(*group), Rev: domain.Revision(*rev), Context: context}
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
