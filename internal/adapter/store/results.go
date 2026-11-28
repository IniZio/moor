package store

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// Page is the page stored with a version of a task's result, domain.ErrNotFound when there is none.
func (s *Store) Page(ctx context.Context, id domain.TaskID, n int) ([]byte, error) {
	var page []byte
	err := s.db.QueryRow(ctx, "SELECT page FROM result_pages WHERE task_id = $1 AND version = $2", string(id), n).Scan(&page)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		return nil, fmt.Errorf("task %s page %d: %w", id, n, domain.ErrNotFound)
	case err != nil:
		return nil, fmt.Errorf("task %s page %d: %w", id, n, err)
	}
	return page, nil
}

// HasNote tells whether the task has a note row with exactly this text.
func (s *Store) HasNote(ctx context.Context, id domain.TaskID, text string) (bool, error) {
	var found bool
	err := s.db.QueryRow(ctx, "SELECT EXISTS (SELECT 1 FROM task_events WHERE task_id = $1 AND kind = 'note' AND data->>'text' = $2)", string(id), text).Scan(&found)
	if err != nil {
		return false, fmt.Errorf("note of %s: %w", id, err)
	}
	return found, nil
}
