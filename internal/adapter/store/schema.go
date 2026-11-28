package store

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// schemaPattern is a plain, lower-case identifier, so it reads the same quoted or not. Postgres keeps pg_ names.
var schemaPattern = regexp.MustCompile(`^[a-z_][a-z0-9_]{0,62}$`)

// CheckSchema reports why name cannot be the schema the portal lives in. The empty name is the database's default.
func CheckSchema(name string) error {
	switch {
	case name == "":
		return nil
	case !schemaPattern.MatchString(name):
		return errors.New("must be 1 to 63 lower-case letters, digits and underscores, not starting with a digit")
	case strings.HasPrefix(name, "pg_"):
		return errors.New("must not start with pg_, which Postgres keeps")
	}
	return nil
}

// poolConfig reads dsn. With a schema, every connection of the pool resolves names in that schema alone.
func poolConfig(dsn, schema string) (*pgxpool.Config, error) {
	cfg, err := pgxpool.ParseConfig(dsn)
	if err != nil {
		return nil, fmt.Errorf("read the database url: %w", err)
	}
	if schema != "" {
		cfg.ConnConfig.RuntimeParams["search_path"] = schema
	}
	return cfg, nil
}

// ensureSchema creates the schema when it is missing. Concurrent CREATE SCHEMA IF NOT EXISTS can still fail on the
// catalog's unique index, so replicas take a lock named after the schema, and the create is skipped when the schema
// is there, which a role without the right to create schemas needs.
func ensureSchema(ctx context.Context, pool *pgxpool.Pool, schema string) error {
	err := pgx.BeginFunc(ctx, pool, func(tx pgx.Tx) error {
		if _, err := tx.Exec(ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))", "moor schema "+schema); err != nil {
			return fmt.Errorf("lock: %w", err)
		}
		var found bool
		if err := tx.QueryRow(ctx, "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)", schema).Scan(&found); err != nil {
			return fmt.Errorf("look for it: %w", err)
		}
		if found {
			return nil
		}
		if _, err := tx.Exec(ctx, "CREATE SCHEMA "+pgx.Identifier{schema}.Sanitize()); err != nil {
			return fmt.Errorf("create it: %w", err)
		}
		return nil
	})
	if err != nil {
		return fmt.Errorf("ensure schema %s: %w", schema, err)
	}
	return nil
}
