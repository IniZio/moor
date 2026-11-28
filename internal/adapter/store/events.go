package store

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// EventsAfter lists up to limit event rows of a task whose seq is above after, in seq order. A task with no such
// rows, or no such task, gives none. Rows commit with the task write that made them, in seq order, so a reader that
// asks again from the last seq it saw misses nothing.
func (s *Store) EventsAfter(ctx context.Context, id domain.TaskID, after int64, limit int) ([]domain.EventRow, error) {
	rows, err := s.pool.Query(ctx, "SELECT seq, at, kind, data FROM task_events WHERE task_id = $1 AND seq > $2 ORDER BY seq LIMIT $3", string(id), after, limit)
	if err != nil {
		return nil, fmt.Errorf("events of %s after %d: %w", id, after, err)
	}
	out, err := pgx.CollectRows(rows, func(row pgx.CollectableRow) (domain.EventRow, error) {
		r := domain.EventRow{Task: id}
		if err := row.Scan(&r.Seq, &r.At, &r.Kind, &r.Data); err != nil {
			return r, fmt.Errorf("scan: %w", err)
		}
		r.At = r.At.UTC()
		return r, nil
	})
	if err != nil {
		return nil, fmt.Errorf("events of %s after %d: %w", id, after, err)
	}
	return out, nil
}

// TaskStamp names the state of the whole task table. It differs after any task is created or written, because a
// task's version only goes up and no task is deleted, so the count and the sum of versions only grow.
func (s *Store) TaskStamp(ctx context.Context) (string, error) {
	var stamp string
	err := s.pool.QueryRow(ctx, "SELECT count(*)::text || '.' || COALESCE(sum(version), 0)::text FROM tasks").Scan(&stamp)
	if err != nil {
		return "", fmt.Errorf("task stamp: %w", err)
	}
	return stamp, nil
}
