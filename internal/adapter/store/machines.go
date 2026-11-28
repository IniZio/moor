package store

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// onMachine is the states in which a task holds a slot on its machine.
const onMachine = "('starting', 'running', 'delivering')"

const selectMachine = `SELECT m.id, COALESCE(m.name, ''), m.enrolled_by, m.pubkey, m.build, m.link_gen, m.holder_addr,
	m.heard_at, m.draining_since, m.revoked_at, m.created_at,
	(SELECT count(*) FROM tasks t WHERE t.runner_id = m.id AND t.state IN ` + onMachine + `)
FROM machines m `

func scanMachine(row pgx.Row) (domain.Machine, error) {
	var m domain.Machine
	var heard, draining, revoked *time.Time
	err := row.Scan(&m.ID, &m.Name, &m.EnrolledBy, &m.PubKey, &m.Build, &m.LinkGen, &m.Holder,
		&heard, &draining, &revoked, &m.CreatedAt, &m.Running)
	if err != nil {
		return domain.Machine{}, fmt.Errorf("scan machine: %w", err)
	}
	m.HeardAt, m.DrainingSince, m.RevokedAt, m.CreatedAt = utc(heard), utc(draining), utc(revoked), m.CreatedAt.UTC()
	return m, nil
}

func (s *Store) AddMachine(ctx context.Context, a domain.MachineAdd) error {
	_, err := s.pool.Exec(ctx, "INSERT INTO machines (id, enrolled_by, join_hash, join_expires_at, created_at) VALUES ($1, $2, $3, $4, $5)",
		string(a.ID), string(a.EnrolledBy), a.JoinHash, a.JoinExpiresAt, a.CreatedAt)
	if err != nil {
		return exists(err, fmt.Sprintf("add machine %s", a.ID))
	}
	return nil
}

// JoinMachine burns the token in the one statement that checks it, so of two joins with one token
// exactly one finds it.
func (s *Store) JoinMachine(ctx context.Context, j domain.MachineJoin) error {
	tag, err := s.pool.Exec(ctx, `UPDATE machines SET name = $3, pubkey = $4, join_hash = NULL, join_expires_at = NULL
WHERE id = $1 AND join_hash = $2 AND join_expires_at > $5 AND revoked_at IS NULL`,
		string(j.ID), j.JoinHash, j.Name, j.PubKey, j.At)
	if err != nil {
		return exists(err, fmt.Sprintf("join machine %s as %s", j.ID, j.Name))
	}
	if tag.RowsAffected() == 0 {
		return fmt.Errorf("join machine %s: %w", j.ID, domain.ErrJoinRefused)
	}
	return nil
}

func (s *Store) Machines(ctx context.Context) ([]domain.Machine, error) {
	rows, err := s.pool.Query(ctx, selectMachine+"ORDER BY m.created_at, m.id")
	if err != nil {
		return nil, fmt.Errorf("list machines: %w", err)
	}
	out, err := pgx.CollectRows(rows, func(r pgx.CollectableRow) (domain.Machine, error) { return scanMachine(r) })
	if err != nil {
		return nil, fmt.Errorf("list machines: %w", err)
	}
	return out, nil
}

func (s *Store) Machine(ctx context.Context, id domain.RunnerID) (domain.Machine, error) {
	m, err := scanMachine(s.pool.QueryRow(ctx, selectMachine+"WHERE m.id = $1", string(id)))
	if errors.Is(err, pgx.ErrNoRows) {
		return domain.Machine{}, fmt.Errorf("machine %s: %w", id, domain.ErrNotFound)
	}
	return m, err
}

func (s *Store) Linked(ctx context.Context, id domain.RunnerID, build, holder string, at time.Time) (int64, error) {
	var gen int64
	err := s.pool.QueryRow(ctx, `UPDATE machines SET link_gen = link_gen + 1, build = $2, holder_addr = $3, heard_at = $4
WHERE id = $1 AND pubkey IS NOT NULL AND revoked_at IS NULL RETURNING link_gen`, string(id), build, holder, at).Scan(&gen)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		return 0, fmt.Errorf("link machine %s: %w", id, domain.ErrUnknownRunner)
	case err != nil:
		return 0, fmt.Errorf("link machine %s: %w", id, err)
	}
	return gen, nil
}

func (s *Store) Heard(ctx context.Context, id domain.RunnerID, gen int64, at time.Time) (bool, error) {
	n, err := s.exec(ctx, "heard machine", "UPDATE machines SET heard_at = $3 WHERE id = $1 AND link_gen = $2 AND revoked_at IS NULL", string(id), gen, at)
	return n == 1, err
}

func (s *Store) Unlinked(ctx context.Context, id domain.RunnerID, gen int64) error {
	_, err := s.exec(ctx, "unlink machine", "UPDATE machines SET holder_addr = '' WHERE id = $1 AND link_gen = $2", string(id), gen)
	return err
}

func (s *Store) DrainMachine(ctx context.Context, id domain.RunnerID, at time.Time) error {
	return s.action(ctx, id, "drain", "UPDATE machines SET draining_since = COALESCE(draining_since, $2) WHERE id = $1", at)
}

func (s *Store) DisconnectMachine(ctx context.Context, id domain.RunnerID) error {
	return s.action(ctx, id, "disconnect", "UPDATE machines SET link_gen = link_gen + 1, holder_addr = '' WHERE id = $1")
}

func (s *Store) RevokeMachine(ctx context.Context, id domain.RunnerID, at time.Time) error {
	return s.action(ctx, id, "revoke", `UPDATE machines SET revoked_at = COALESCE(revoked_at, $2), link_gen = link_gen + 1,
	holder_addr = '', join_hash = NULL, join_expires_at = NULL WHERE id = $1`, at)
}

func (s *Store) RemoveMachine(ctx context.Context, id domain.RunnerID) error {
	n, err := s.exec(ctx, "remove machine", "DELETE FROM machines WHERE id = $1 AND revoked_at IS NOT NULL", string(id))
	if err != nil || n == 1 {
		return err
	}
	if _, err := s.Machine(ctx, id); err != nil {
		return err
	}
	return fmt.Errorf("remove machine %s: %w", id, domain.ErrNotRevoked)
}

// action runs a row action and returns domain.ErrNotFound when no machine has the id.
func (s *Store) action(ctx context.Context, id domain.RunnerID, what, sql string, args ...any) error {
	n, err := s.exec(ctx, what+" machine", sql, append([]any{string(id)}, args...)...)
	if err == nil && n == 0 {
		return fmt.Errorf("%s machine %s: %w", what, id, domain.ErrNotFound)
	}
	return err
}

func (s *Store) exec(ctx context.Context, what, sql string, args ...any) (int64, error) {
	tag, err := s.pool.Exec(ctx, sql, args...)
	if err != nil {
		return 0, fmt.Errorf("%s: %w", what, err)
	}
	return tag.RowsAffected(), nil
}

func (s *Store) WaitingTasks(ctx context.Context, limit int) ([]domain.TaskID, error) {
	rows, err := s.pool.Query(ctx, "SELECT id FROM tasks WHERE state = 'waiting' ORDER BY created_at, id LIMIT $1", limit)
	if err != nil {
		return nil, fmt.Errorf("waiting tasks: %w", err)
	}
	ids, err := pgx.CollectRows(rows, pgx.RowTo[domain.TaskID])
	if err != nil {
		return nil, fmt.Errorf("waiting tasks: %w", err)
	}
	return ids, nil
}

func (s *Store) TasksOn(ctx context.Context, id domain.RunnerID) ([]domain.Task, error) {
	rows, err := s.pool.Query(ctx, selectTask+"WHERE runner_id = $1 AND state IN "+onMachine+" ORDER BY created_at, id", string(id))
	if err != nil {
		return nil, fmt.Errorf("tasks on %s: %w", id, err)
	}
	out, err := pgx.CollectRows(rows, func(r pgx.CollectableRow) (domain.Task, error) { return scanTask(r) })
	if err != nil {
		return nil, fmt.Errorf("tasks on %s: %w", id, err)
	}
	return out, nil
}
