package store

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/jackc/pgx/v5/stdlib"

	"github.com/IniZio/moor/internal/domain"
)

// Store keeps tasks in Postgres. Any number of replicas may share one database.
type Store struct {
	pool *pgxpool.Pool
}

// Open connects to the database named by dsn and applies the pending migrations.
func Open(ctx context.Context, dsn string) (*Store, error) {
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		return nil, fmt.Errorf("open store: %w", err)
	}
	db := stdlib.OpenDBFromPool(pool)
	err = migrate(ctx, db)
	if closeErr := db.Close(); closeErr != nil && err == nil {
		err = fmt.Errorf("close migration handle: %w", closeErr)
	}
	if err != nil {
		pool.Close()
		return nil, err
	}
	return &Store{pool: pool}, nil
}

func (s *Store) Close() { s.pool.Close() }

// querier is what a pool and a transaction share.
type querier interface {
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

func (s *Store) Load(ctx context.Context, id domain.TaskID) (domain.Task, error) {
	return loadTask(ctx, s.pool, id)
}

func loadTask(ctx context.Context, q querier, id domain.TaskID) (domain.Task, error) {
	t, err := scanTask(q.QueryRow(ctx, selectTask+"WHERE id = $1", string(id)))
	if errors.Is(err, pgx.ErrNoRows) {
		return domain.Task{}, fmt.Errorf("task %s: %w", id, domain.ErrNotFound)
	}
	return t, err
}

func (s *Store) ByKey(ctx context.Context, key domain.TaskKey) (domain.Task, error) {
	row := s.pool.QueryRow(ctx, selectByKey, string(key.Project), string(key.Source), string(key.Group), string(key.Rev))
	t, err := scanTask(row)
	if errors.Is(err, pgx.ErrNoRows) {
		return domain.Task{}, fmt.Errorf("key %v: %w", key, domain.ErrNotFound)
	}
	return t, err
}

func (s *Store) LiveInGroup(ctx context.Context, project domain.ProjectName, source domain.TriggerSource, group domain.TriggerGroup) ([]domain.Task, error) {
	rows, err := s.pool.Query(ctx, selectLive, string(project), string(source), string(group), liveStates())
	if err != nil {
		return nil, fmt.Errorf("live tasks of %s %s %s: %w", project, source, group, err)
	}
	defer rows.Close()
	var live []domain.Task
	for rows.Next() {
		t, err := scanTask(rows)
		if err != nil {
			return nil, err
		}
		live = append(live, t)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("live tasks of %s %s %s: %w", project, source, group, err)
	}
	return live, nil
}

// Both reads end on the columns of the unique trigger key, so they walk its index.
const (
	selectKeyed = "WHERE project = $1 AND trigger_source = $2 AND trigger_group = $3"
)

var (
	selectByKey = selectTask + selectKeyed + " AND trigger_rev = $4"
	selectLive  = selectTask + selectKeyed + " AND state = ANY($4) ORDER BY created_at, id"
)

func liveStates() []string {
	var names []string
	for s := domain.Waiting; s <= domain.Cancelled; s++ {
		if s.Live() {
			names = append(names, s.String())
		}
	}
	return names
}

func (s *Store) Versions(ctx context.Context, id domain.TaskID) ([]domain.ResultVersion, error) {
	rows, err := s.pool.Query(ctx, "SELECT version, entry_url, line, digest, published_at FROM result_versions WHERE task_id = $1 ORDER BY version", string(id))
	if err != nil {
		return nil, fmt.Errorf("versions of %s: %w", id, err)
	}
	defer rows.Close()
	var out []domain.ResultVersion
	for rows.Next() {
		v := domain.ResultVersion{Task: id}
		if err := rows.Scan(&v.N, &v.EntryURL, &v.Line, &v.Digest, &v.PublishedAt); err != nil {
			return nil, fmt.Errorf("versions of %s: %w", id, err)
		}
		v.PublishedAt = v.PublishedAt.UTC()
		out = append(out, v)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("versions of %s: %w", id, err)
	}
	return out, nil
}

const uniqueViolation = "23505"

// exists turns a unique violation into domain.ErrExists and leaves any other error as it is.
func exists(err error, what string) error {
	var pg *pgconn.PgError
	if errors.As(err, &pg) && pg.Code == uniqueViolation {
		return fmt.Errorf("%s: %w", what, domain.ErrExists)
	}
	return fmt.Errorf("%s: %w", what, err)
}
