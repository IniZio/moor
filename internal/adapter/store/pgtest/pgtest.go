// Package pgtest gives a test its own schema on the Postgres that MOOR_TEST_POSTGRES_DSN names.
package pgtest

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"net/url"
	"os"
	"testing"

	"github.com/jackc/pgx/v5"
)

// DSNEnv names the server the tests run against.
const DSNEnv = "MOOR_TEST_POSTGRES_DSN"

// SchemaDSN returns a DSN whose search_path is a fresh schema, dropped when the test ends, so tests share a
// server and nothing else. It skips the test when no server is named: make test-store sets one.
func SchemaDSN(t *testing.T) string {
	t.Helper()
	dsn := os.Getenv(DSNEnv)
	if dsn == "" {
		t.Skipf("%s is not set: these tests need a Postgres, run them with make test-store", DSNEnv)
	}
	suffix := make([]byte, 6)
	if _, err := rand.Read(suffix); err != nil {
		t.Fatal(err)
	}
	schema := "t_" + hex.EncodeToString(suffix)
	ctx := context.Background()
	admin, err := pgx.Connect(ctx, dsn)
	if err != nil {
		t.Fatalf("connect to %s: %v", DSNEnv, err)
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
		t.Fatalf("%s must be a postgres:// URL: %v", DSNEnv, err)
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	return u.String()
}
