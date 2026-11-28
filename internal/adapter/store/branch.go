package store

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// DefaultBranch is the default branch the portal read for the task, and whether it read one. An empty branch that
// was read is a read that failed.
func (s *Store) DefaultBranch(ctx context.Context, task domain.TaskID) (string, bool, error) {
	var b *string
	err := s.pool.QueryRow(ctx, "SELECT default_branch FROM tasks WHERE id = $1", string(task)).Scan(&b)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", false, fmt.Errorf("task %s: %w", task, domain.ErrNotFound)
	}
	if err != nil {
		return "", false, fmt.Errorf("default branch of %s: %w", task, err)
	}
	if b == nil {
		return "", false, nil
	}
	return *b, true, nil
}

// SetDefaultBranch keeps the branch read for the task. The first read stays.
func (s *Store) SetDefaultBranch(ctx context.Context, task domain.TaskID, branch string) error {
	if _, err := s.pool.Exec(ctx, "UPDATE tasks SET default_branch = $2 WHERE id = $1 AND default_branch IS NULL", string(task), branch); err != nil {
		return fmt.Errorf("keep the default branch of %s: %w", task, err)
	}
	return nil
}
