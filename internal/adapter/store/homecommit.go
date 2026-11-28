package store

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// writeHomeCommit records the home commit a created task started with, as the first of its commits.
func writeHomeCommit(ctx context.Context, tx pgx.Tx, w domain.TaskWrite) error {
	if !w.Create || w.Task.HomeCommit == "" {
		return nil
	}
	_, err := tx.Exec(ctx, "INSERT INTO task_home_commits (task_id, n, commit, at, cause) VALUES ($1, 1, $2, $3, 'start')",
		string(w.Task.ID), string(w.Task.HomeCommit), w.Task.CreatedAt)
	if err != nil {
		return fmt.Errorf("task %s home commit: %w", w.Task.ID, err)
	}
	return nil
}

// newestHomeCommit reads the latest home commit the task used, empty when it has none.
func newestHomeCommit(ctx context.Context, q querier, id domain.TaskID) (domain.Commit, error) {
	var commit domain.Commit
	err := q.QueryRow(ctx, "SELECT commit FROM task_home_commits WHERE task_id = $1 ORDER BY n DESC LIMIT 1", string(id)).Scan(&commit)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("task %s home commit: %w", id, err)
	}
	return commit, nil
}
