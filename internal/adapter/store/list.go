package store

import (
	"context"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

const defaultListLimit = 200

var replacedBy = `CASE WHEN state = 'superseded' THEN (
	SELECT n.id FROM tasks n
	WHERE n.repo_host = t.repo_host AND n.repo_path = t.repo_path AND n.spec_name = t.spec_name
		AND n.event_source = t.event_source AND n.event_group = t.event_group AND n.created_at > t.created_at
	ORDER BY n.created_at, length(n.id), n.id LIMIT 1) END`

// The filters are optional parameters: a NULL one narrows nothing, so one statement serves every combination.
var listSQL = "SELECT " + strings.Join(taskColumns, ", ") + ", " + replacedBy + ` AS replaced_by FROM tasks t
WHERE ($1::text IS NULL OR spec_name = $1)
	AND (NOT $2::boolean OR spec_name IS NULL)
	AND ($3::text IS NULL OR (repo_host = $3 AND repo_path = $4::text))
	AND ($5::text IS NULL OR started_by = $5)
	AND (NOT $6::boolean OR starts_with(started_by, $7::text))
	AND ($8::text[] IS NULL OR state = ANY($8))
ORDER BY created_at DESC, length(id) DESC, id DESC
LIMIT $9`

const startersSQL = `SELECT started_by FROM tasks
WHERE ($1::text IS NULL OR spec_name = $1)
	AND (NOT $2::boolean OR spec_name IS NULL)
	AND started_by <> '' AND NOT starts_with(started_by, $3::text)
GROUP BY started_by
ORDER BY lower(started_by), started_by`

func textOrNil(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// listArgs lists the parameters of listSQL for f, with NULL for each filter that is left empty.
func listArgs(f domain.TaskFilter, limit int) []any {
	var host, path *string
	if f.Repo.Complete() {
		h, p := string(f.Repo.Host), string(f.Repo.Path)
		host, path = &h, &p
	}
	var states []string
	if f.Group != "" {
		states = []string{}
		for _, st := range f.Group.States() {
			states = append(states, st.String())
		}
	}
	return []any{textOrNil(string(f.Spec)), f.FromPrompt, host, path, textOrNil(f.StartedBy), f.ByEvent, domain.EventStarterPrefix, states, limit + 1}
}

// ListTasks lists the tasks that match f, newest first.
func (s *Store) ListTasks(ctx context.Context, f domain.TaskFilter) (domain.TaskList, error) {
	limit := f.Limit
	if limit <= 0 {
		limit = defaultListLimit
	}
	rows, err := s.pool.Query(ctx, listSQL, listArgs(f, limit)...)
	if err != nil {
		return domain.TaskList{}, fmt.Errorf("list tasks: %w", err)
	}
	defer rows.Close()
	var out domain.TaskList
	for rows.Next() {
		var replaced *string
		t, err := scanTaskWith(rows, &replaced)
		if err != nil {
			return domain.TaskList{}, err
		}
		it := domain.TaskListing{Task: t}
		if replaced != nil {
			it.ReplacedBy = domain.TaskID(*replaced)
		}
		out.Items = append(out.Items, it)
	}
	if err := rows.Err(); err != nil {
		return domain.TaskList{}, fmt.Errorf("list tasks: %w", err)
	}
	if len(out.Items) > limit {
		out.Items, out.More = out.Items[:limit], true
	}
	return out, nil
}

// Starters lists the people who started tasks in the scope of f, which is its Spec and FromPrompt, sorted
// without case. A task an event started with nobody running it by hand has no person.
func (s *Store) Starters(ctx context.Context, f domain.TaskFilter) ([]string, error) {
	rows, err := s.pool.Query(ctx, startersSQL, textOrNil(string(f.Spec)), f.FromPrompt, domain.EventStarterPrefix)
	if err != nil {
		return nil, fmt.Errorf("list starters: %w", err)
	}
	people, err := pgx.CollectRows(rows, pgx.RowTo[string])
	if err != nil {
		return nil, fmt.Errorf("list starters: %w", err)
	}
	return people, nil
}

// NextTaskID gives the next task number. Numbers are never reused, but a number may go unused.
func (s *Store) NextTaskID(ctx context.Context) (domain.TaskID, error) {
	var n int64
	if err := s.pool.QueryRow(ctx, "SELECT nextval('task_numbers')").Scan(&n); err != nil {
		return "", fmt.Errorf("next task number: %w", err)
	}
	return domain.TaskID(fmt.Sprint(n)), nil
}
