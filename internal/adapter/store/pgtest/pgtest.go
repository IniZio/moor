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

// BaseDSN returns the DSN the server is named by, skipping the test when none is named: make test-store sets one.
func BaseDSN(t *testing.T) string {
	t.Helper()
	dsn := os.Getenv(DSNEnv)
	if dsn == "" {
		t.Skipf("%s is not set: these tests need a Postgres, run them with make test-store", DSNEnv)
	}
	return dsn
}

// Name returns a schema name no test shares and drops that schema, with whatever the test put in it, when the test
// ends. It does not create the schema.
func Name(t *testing.T) string {
	t.Helper()
	dsn := BaseDSN(t)
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
	t.Cleanup(func() {
		_, _ = admin.Exec(context.Background(), "DROP SCHEMA IF EXISTS "+schema+" CASCADE")
		_ = admin.Close(context.Background())
	})
	return schema
}

// SchemaDSN returns a DSN whose search_path is a fresh schema, dropped when the test ends, so tests share a
// server and nothing else.
func SchemaDSN(t *testing.T) string {
	t.Helper()
	dsn := BaseDSN(t)
	schema := Name(t)
	ctx := context.Background()
	admin, err := pgx.Connect(ctx, dsn)
	if err != nil {
		t.Fatalf("connect to %s: %v", DSNEnv, err)
	}
	defer func() { _ = admin.Close(ctx) }()
	if _, err := admin.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	u, err := url.Parse(dsn)
	if err != nil {
		t.Fatalf("%s must be a postgres:// URL: %v", DSNEnv, err)
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	return u.String()
}
