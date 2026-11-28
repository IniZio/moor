package store

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// RecordBlocked inserts the row or counts one more on the row of the same task, proxy, host, port and method.
// A decision already made stays.
func (s *Store) RecordBlocked(ctx context.Context, h domain.BlockedHit) error {
	_, err := s.pool.Exec(ctx, `INSERT INTO blocked_requests (task_id, proxy, host, port, method, reason, first_at, last_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
		ON CONFLICT (task_id, proxy, host, port, method)
		DO UPDATE SET count = blocked_requests.count + 1, last_at = EXCLUDED.last_at, reason = EXCLUDED.reason`,
		string(h.Task), string(h.Proxy), h.Host, h.Port, string(h.Method), string(h.Reason), h.At.UTC())
	if err != nil {
		return fmt.Errorf("record blocked request: %w", err)
	}
	return nil
}

const selectBlocked = "SELECT id, task_id, proxy, host, port, method, reason, count, first_at, last_at, decision, decided_by, decided_at FROM blocked_requests "

func scanBlocked(row pgx.Row) (domain.BlockedRequest, error) {
	var b domain.BlockedRequest
	var decided *time.Time
	err := row.Scan(&b.ID, &b.Task, &b.Proxy, &b.Host, &b.Port, &b.Method, &b.Reason, &b.Count, &b.FirstAt, &b.LastAt, &b.Decision, &b.DecidedBy, &decided)
	if err != nil {
		return domain.BlockedRequest{}, fmt.Errorf("read blocked request: %w", err)
	}
	b.FirstAt, b.LastAt = b.FirstAt.UTC(), b.LastAt.UTC()
	if decided != nil {
		b.DecidedAt = decided.UTC()
	}
	return b, nil
}

func (s *Store) BlockedRequests(ctx context.Context, task domain.TaskID) ([]domain.BlockedRequest, error) {
	rows, err := s.pool.Query(ctx, selectBlocked+"WHERE task_id = $1 ORDER BY id", string(task))
	if err != nil {
		return nil, fmt.Errorf("blocked requests of %s: %w", task, err)
	}
	defer rows.Close()
	var out []domain.BlockedRequest
	for rows.Next() {
		b, err := scanBlocked(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, b)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("blocked requests of %s: %w", task, err)
	}
	return out, nil
}

func (s *Store) BlockedRequest(ctx context.Context, id domain.BlockedID) (domain.BlockedRequest, error) {
	b, err := scanBlocked(s.pool.QueryRow(ctx, selectBlocked+"WHERE id = $1", int64(id)))
	if errors.Is(err, pgx.ErrNoRows) {
		return domain.BlockedRequest{}, fmt.Errorf("blocked request %d: %w", id, domain.ErrNotFound)
	}
	return b, err
}

func (s *Store) DecideBlocked(ctx context.Context, id domain.BlockedID, d domain.Decision, by string, at time.Time) error {
	tag, err := s.pool.Exec(ctx, "UPDATE blocked_requests SET decision = $2, decided_by = $3, decided_at = $4 WHERE id = $1",
		int64(id), string(d), by, at.UTC())
	switch {
	case err != nil:
		return fmt.Errorf("decide blocked request %d: %w", id, err)
	case tag.RowsAffected() == 0:
		return fmt.Errorf("blocked request %d: %w", id, domain.ErrNotFound)
	}
	return nil
}
