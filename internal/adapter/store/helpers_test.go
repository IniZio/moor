package store

import (
	"context"
	"fmt"
	"testing"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/domain/storetest"
)

func schemaDSN(t *testing.T) string { return pgtest.SchemaDSN(t) }

func openStore(t *testing.T, dsn string) *Store {
	t.Helper()
	s, err := Open(context.Background(), dsn)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	t.Cleanup(s.Close)
	return s
}

func freshStore(t *testing.T) *Store { return openStore(t, schemaDSN(t)) }

func (s *Store) events(ctx context.Context, id domain.TaskID) ([]domain.EventRow, error) {
	rows, err := s.pool.Query(ctx, "SELECT task_id, seq, at, kind, data FROM task_events WHERE task_id = $1 ORDER BY seq", string(id))
	if err != nil {
		return nil, fmt.Errorf("events of %s: %w", id, err)
	}
	defer rows.Close()
	var out []domain.EventRow
	for rows.Next() {
		var r domain.EventRow
		if err := rows.Scan(&r.Task, &r.Seq, &r.At, &r.Kind, &r.Data); err != nil {
			return nil, fmt.Errorf("events of %s: %w", id, err)
		}
		out = append(out, r)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("events of %s: %w", id, err)
	}
	return out, nil
}

func subject(t *testing.T) storetest.Subject {
	s := freshStore(t)
	return storetest.Subject{Store: s, Events: s.events}
}

func count(t *testing.T, s *Store, query string, args ...any) int {
	t.Helper()
	var n int
	if err := s.pool.QueryRow(context.Background(), query, args...).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}
