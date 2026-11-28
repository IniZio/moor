package store

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// Save applies w in one transaction. The task row stays locked from the version check to the
// commit, so two writers of one task take turns and the second finds a newer version.
func (s *Store) Save(ctx context.Context, w domain.TaskWrite) (domain.Task, error) {
	var saved domain.Task
	err := pgx.BeginFunc(ctx, s.db, func(tx pgx.Tx) error {
		var err error
		saved, err = save(ctx, tx, w)
		return err
	})
	if err != nil {
		return domain.Task{}, fmt.Errorf("save task %s: %w", w.Task.ID, err)
	}
	return saved, nil
}

func save(ctx context.Context, tx pgx.Tx, w domain.TaskWrite) (domain.Task, error) {
	var lastSeq int64
	if !w.Create {
		var err error
		if lastSeq, err = lockRow(ctx, tx, w.Task); err != nil {
			return domain.Task{}, err
		}
	}
	if err := checkEvents(w, lastSeq); err != nil {
		return domain.Task{}, err
	}
	if err := writeTask(ctx, tx, w); err != nil {
		return domain.Task{}, err
	}
	if err := writeEvents(ctx, tx, w.Events); err != nil {
		return domain.Task{}, err
	}
	if w.Result != nil {
		if err := writeResult(ctx, tx, w.Task.ID, *w.Result); err != nil {
			return domain.Task{}, err
		}
	}
	return loadTask(ctx, tx, w.Task.ID)
}

// lockRow locks the stored row and refuses a writer that loaded an older version.
func lockRow(ctx context.Context, tx pgx.Tx, t domain.Task) (int64, error) {
	var version, lastSeq int64
	err := tx.QueryRow(ctx, "SELECT version, last_seq FROM tasks WHERE id = $1 FOR UPDATE", string(t.ID)).Scan(&version, &lastSeq)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		return 0, fmt.Errorf("task %s: %w", t.ID, domain.ErrNotFound)
	case err != nil:
		return 0, fmt.Errorf("lock task %s: %w", t.ID, err)
	case version != t.Version:
		return 0, fmt.Errorf("task %s at version %d, write has %d: %w", t.ID, version, t.Version, domain.ErrStaleVersion)
	}
	return lastSeq, nil
}

// checkEvents wants the rows to continue the stored seq with no gap and the task's last_seq to
// name the last of them. The version check has made the writer's view the stored one.
func checkEvents(w domain.TaskWrite, lastSeq int64) error {
	seq := lastSeq
	for _, ev := range w.Events {
		seq++
		if ev.Seq != seq || ev.Task != w.Task.ID {
			return fmt.Errorf("task %s event seq %d, want %d", w.Task.ID, ev.Seq, seq)
		}
	}
	if w.Task.LastSeq != seq {
		return fmt.Errorf("task %s last_seq %d, events end at %d", w.Task.ID, w.Task.LastSeq, seq)
	}
	return nil
}

func writeTask(ctx context.Context, tx pgx.Tx, w domain.TaskWrite) error {
	args, err := taskArgs(w.Task, w.Task.Version+1)
	if err != nil {
		return err
	}
	sql := updateTask
	if w.Create {
		sql = insertTask
	}
	if _, err := tx.Exec(ctx, sql, args...); err != nil {
		return exists(err, fmt.Sprintf("task %s", w.Task.ID))
	}
	return nil
}

func writeEvents(ctx context.Context, tx pgx.Tx, rows []domain.EventRow) error {
	if len(rows) == 0 {
		return nil
	}
	var batch pgx.Batch
	for _, r := range rows {
		batch.Queue("INSERT INTO task_events (task_id, seq, at, kind, data) VALUES ($1, $2, $3, $4, $5)",
			string(r.Task), r.Seq, r.At, string(r.Kind), nilIfEmpty(r.Data))
	}
	results := tx.SendBatch(ctx, &batch)
	for _, r := range rows {
		if _, err := results.Exec(); err != nil {
			_ = results.Close()
			return fmt.Errorf("task %s event %d: %w", r.Task, r.Seq, err)
		}
	}
	if err := results.Close(); err != nil {
		return fmt.Errorf("task events: %w", err)
	}
	return nil
}

// writeResult wants the next version number. A digest the task has published is refused, so
// publishing the same page twice stores it once.
func writeResult(ctx context.Context, tx pgx.Tx, id domain.TaskID, r domain.ResultVersion) error {
	var latest int
	if err := tx.QueryRow(ctx, "SELECT COALESCE(max(version), 0) FROM result_versions WHERE task_id = $1", string(id)).Scan(&latest); err != nil {
		return fmt.Errorf("task %s versions: %w", id, err)
	}
	what := fmt.Sprintf("task %s result %d", id, r.N)
	if r.N != latest+1 {
		return fmt.Errorf("%s: %w", what, domain.ErrExists)
	}
	_, err := tx.Exec(ctx, "INSERT INTO result_versions (task_id, version, entry_url, line, digest, published_at) VALUES ($1, $2, $3, $4, $5, $6)",
		string(id), r.N, r.EntryURL, r.Line, r.Digest, r.PublishedAt)
	if err != nil {
		return exists(err, what)
	}
	return nil
}
