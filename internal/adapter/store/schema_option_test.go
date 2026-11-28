package store

import (
	"context"
	"io/fs"
	"net/url"
	"sort"
	"strings"
	"sync"
	"testing"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/domain/storetest"
)

func TestSchemaNamesArePlainIdentifiers(t *testing.T) {
	for _, ok := range []string{"", "moor", "portal_2", "_x", "public", strings.Repeat("a", 63)} {
		if err := CheckSchema(ok); err != nil {
			t.Errorf("CheckSchema(%q) = %v, want nil", ok, err)
		}
	}
	for _, bad := range []string{"Moor", "1moor", "moor-db", "moor.db", "moor db", `"moor"`, "moor;drop", "pg_moor", "pg_catalog", strings.Repeat("a", 64), "moör"} {
		if err := CheckSchema(bad); err == nil {
			t.Errorf("CheckSchema(%q) accepted it", bad)
		}
	}
	if _, err := OpenIn(context.Background(), "postgres://unused", "Bad Name"); err == nil || !strings.Contains(err.Error(), "database-schema") {
		t.Errorf("OpenIn with a bad name = %v", err)
	}
}

// openIn opens a store in a schema the test names and the portal creates.
func openIn(t *testing.T, schema string) *Store {
	t.Helper()
	s, err := OpenIn(context.Background(), pgtest.BaseDSN(t), schema)
	if err != nil {
		t.Fatalf("open in %s: %v", schema, err)
	}
	t.Cleanup(s.Close)
	return s
}

func TestPostgresPassesTheStoreContractInASchemaItCreates(t *testing.T) {
	storetest.Run(t, func(t *testing.T) storetest.Subject {
		s := openIn(t, pgtest.Name(t))
		return storetest.Subject{Store: s, Events: s.events, Page: s.Page}
	})
	storetest.RunSettings(t, func(t *testing.T) storetest.SettingsStore { return openIn(t, pgtest.Name(t)) })
}

// snapshot is every object of a schema, its columns and the rows of its two tables, as text.
func snapshot(t *testing.T, dsn, schema string) string {
	t.Helper()
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = conn.Close(ctx) }()
	out := lines(t, conn, "object", `SELECT c.relkind::text || ' ' || c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1`, schema)
	out = append(out, lines(t, conn, "column", `SELECT table_name || '.' || column_name || ' ' || data_type FROM information_schema.columns WHERE table_schema = $1`, schema)...)
	for _, table := range []string{"tasks", "goose_db_version"} {
		q := "SELECT coalesce(string_agg(x::text, '|' ORDER BY x::text), '') FROM " + pgx.Identifier{schema, table}.Sanitize() + " x"
		out = append(out, lines(t, conn, "rows "+table, q)...)
	}
	sort.Strings(out)
	return strings.Join(out, "\n")
}

func lines(t *testing.T, conn *pgx.Conn, label, query string, args ...any) []string {
	t.Helper()
	rows, err := conn.Query(context.Background(), query, args...)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var s string
		if err := rows.Scan(&s); err != nil {
			t.Fatal(err)
		}
		out = append(out, label+" "+s)
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	return out
}

// A database another program shares already holds a tasks table and a goose_db_version of its own.
func TestASchemaLeavesTheDefaultOnesAloneAndKeepsItsOwnVersionTable(t *testing.T) {
	legacyDSN := schemaDSN(t)
	legacy := searchPath(t, legacyDSN)
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, legacyDSN)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = conn.Close(ctx) }()
	for _, stmt := range []string{
		"CREATE TABLE tasks (id text PRIMARY KEY, junk text)",
		"INSERT INTO tasks VALUES ('theirs', 'not ours')",
		"CREATE TABLE goose_db_version (id integer PRIMARY KEY, version_id bigint NOT NULL, is_applied boolean NOT NULL, tstamp timestamp NOT NULL DEFAULT now())",
		"INSERT INTO goose_db_version VALUES (1, 0, true), (2, 999, true)",
	} {
		if _, err := conn.Exec(ctx, stmt); err != nil {
			t.Fatal(err)
		}
	}
	before := snapshot(t, legacyDSN, legacy)

	ours := pgtest.Name(t)
	s, err := OpenIn(ctx, legacyDSN, ours)
	if err != nil {
		t.Fatalf("open in %s: %v", ours, err)
	}
	defer s.Close()
	mustCreate(t, s, newTask("t1", "r1"))
	if got, err := s.Load(ctx, domain.TaskID("t1")); err != nil || got.ID != "t1" {
		t.Fatalf("a task in %s = %+v, %v", ours, got, err)
	}
	if after := snapshot(t, legacyDSN, legacy); after != before {
		t.Errorf("the default schema changed:\nbefore:\n%s\nafter:\n%s", before, after)
	}
	if n, want := count(t, s, "SELECT count(*) FROM "+ours+".goose_db_version WHERE version_id > 0"), len(migrationNames(t)); n != want {
		t.Errorf("%s.goose_db_version holds %d migrations, want %d", ours, n, want)
	}
	if n := count(t, s, "SELECT count(*) FROM information_schema.tables WHERE table_schema = $1 AND table_name = 'tasks'", ours); n != 1 {
		t.Errorf("tasks tables in %s: %d, want 1", ours, n)
	}
}

func searchPath(t *testing.T, dsn string) string {
	t.Helper()
	u, err := url.Parse(dsn)
	if err != nil {
		t.Fatal(err)
	}
	return u.Query().Get("search_path")
}

func migrationNames(t *testing.T) []string {
	t.Helper()
	names, err := fs.Glob(migrationFiles, "migrations/*.sql")
	if err != nil || len(names) == 0 {
		t.Fatalf("migrations: %v, %v", names, err)
	}
	return names
}

// Replicas that start together on a schema nobody has made yet make it once and migrate once.
func TestReplicasStartingTogetherMakeTheSchemaAndMigrateOnce(t *testing.T) {
	ours := pgtest.Name(t)
	dsn := pgtest.BaseDSN(t)
	const replicas = 6
	var wg sync.WaitGroup
	stores := make([]*Store, replicas)
	errs := make([]error, replicas)
	start := make(chan struct{})
	for i := range replicas {
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			stores[i], errs[i] = OpenIn(context.Background(), dsn, ours)
		}()
	}
	close(start)
	wg.Wait()
	for i, err := range errs {
		if err != nil {
			t.Fatalf("replica %d: %v", i, err)
		}
		defer stores[i].Close()
	}
	n := count(t, stores[0], "SELECT count(*) FROM "+ours+".goose_db_version WHERE version_id > 0")
	if want := len(migrationNames(t)); n != want {
		t.Errorf("%d migrations recorded, want %d", n, want)
	}
}
