package store

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net/url"
	"os"
	"testing"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/domain/storetest"
)

const dsnEnv = "MOOR_TEST_POSTGRES_DSN"

// schemaDSN gives the test its own schema, so tests share a server and nothing else. This is the
// only skip in the package: without a server the store has nothing to run against.
func schemaDSN(t *testing.T) string {
	t.Helper()
	dsn := os.Getenv(dsnEnv)
	if dsn == "" {
		t.Skipf("%s is not set: these tests need a Postgres, run them with make test-store", dsnEnv)
	}
	suffix := make([]byte, 6)
	if _, err := rand.Read(suffix); err != nil {
		t.Fatal(err)
	}
	schema := "t_" + hex.EncodeToString(suffix)
	ctx := context.Background()
	admin, err := pgx.Connect(ctx, dsn)
	if err != nil {
		t.Fatalf("connect to %s: %v", dsnEnv, err)
	}
	if _, err := admin.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = admin.Exec(context.Background(), "DROP SCHEMA "+schema+" CASCADE")
		_ = admin.Close(context.Background())
	})
	u, err := url.Parse(dsn)
	if err != nil {
		t.Fatalf("%s must be a postgres:// URL: %v", dsnEnv, err)
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	return u.String()
}

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
