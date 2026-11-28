package store

import (
	"context"
	"database/sql"
	"embed"
	"fmt"
	"io/fs"

	"github.com/pressly/goose/v3"
	"github.com/pressly/goose/v3/lock"
)

//go:embed migrations/*.sql
var migrationFiles embed.FS

// migrate applies the migrations not yet recorded. Goose takes a Postgres advisory lock for the
// run, so replicas that start together apply each one once and the rest find nothing to do. With a schema,
// goose's version table is that schema's too.
func migrate(ctx context.Context, db *sql.DB, schema string) error {
	files, err := fs.Sub(migrationFiles, "migrations")
	if err != nil {
		return fmt.Errorf("migrations: %w", err)
	}
	locker, err := lock.NewPostgresSessionLocker(lock.WithLockTimeout(1, 120))
	if err != nil {
		return fmt.Errorf("migration lock: %w", err)
	}
	opts := []goose.ProviderOption{goose.WithSessionLocker(locker)}
	if schema != "" {
		opts = append(opts, goose.WithTableName(schema+".goose_db_version"))
	}
	provider, err := goose.NewProvider(goose.DialectPostgres, db, files, opts...)
	if err != nil {
		return fmt.Errorf("migrations: %w", err)
	}
	if _, err := provider.Up(ctx); err != nil {
		return fmt.Errorf("apply migrations: %w", err)
	}
	return nil
}
