package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

// notFound turns no row into domain.ErrNotFound and wraps any other error.
func notFound(err error, what string) error {
	if errors.Is(err, pgx.ErrNoRows) {
		return fmt.Errorf("%s: %w", what, domain.ErrNotFound)
	}
	return fmt.Errorf("%s: %w", what, err)
}

// Locked runs fn with the delivery of a place, an empty one when there is none yet, while no other caller on
// any replica holds the same place, and stores what fn returns. The lock is a Postgres advisory lock that the
// transaction holds, so a second caller waits for the first to commit and then reads what it wrote. fn may
// call out to the source: a failure rolls the write back.
func (s *Store) Locked(ctx context.Context, kind, ref string, fn func(domain.Delivery) (domain.Delivery, error)) error {
	err := pgx.BeginFunc(ctx, s.db, func(tx pgx.Tx) error {
		if _, err := tx.Exec(ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))", kind+"|"+ref); err != nil {
			return fmt.Errorf("lock delivery %s: %w", ref, err)
		}
		d, err := readDelivery(ctx, tx, kind, ref)
		if err != nil {
			return err
		}
		out, err := fn(d)
		if err != nil {
			return err
		}
		return writeDelivery(ctx, tx, out)
	})
	if err != nil {
		return fmt.Errorf("delivery %s: %w", ref, err)
	}
	return nil
}

func readDelivery(ctx context.Context, q querier, kind, ref string) (domain.Delivery, error) {
	d := domain.Delivery{Kind: kind, Ref: ref}
	var sections []byte
	err := q.QueryRow(ctx, "SELECT external_id, sections, updated_at FROM deliveries WHERE target_kind = $1 AND target_ref = $2", kind, ref).
		Scan(&d.ExternalID, &sections, &d.UpdatedAt)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		return d, nil
	case err != nil:
		return d, fmt.Errorf("read delivery %s: %w", ref, err)
	}
	d.UpdatedAt = d.UpdatedAt.UTC()
	if err := json.Unmarshal(sections, &d.Sections); err != nil {
		return d, fmt.Errorf("sections of delivery %s: %w", ref, err)
	}
	return d, nil
}

func writeDelivery(ctx context.Context, tx pgx.Tx, d domain.Delivery) error {
	sections, err := json.Marshal(d.Sections)
	if err != nil {
		return fmt.Errorf("encode sections of delivery %s: %w", d.Ref, err)
	}
	if d.UpdatedAt.IsZero() {
		d.UpdatedAt = time.Now()
	}
	_, err = tx.Exec(ctx, `INSERT INTO deliveries (target_kind, target_ref, external_id, sections, updated_at) VALUES ($1, $2, $3, $4, $5)
		ON CONFLICT (target_kind, target_ref) DO UPDATE SET external_id = $3, sections = $4, updated_at = $5`,
		d.Kind, d.Ref, d.ExternalID, sections, d.UpdatedAt.UTC())
	if err != nil {
		return fmt.Errorf("write delivery %s: %w", d.Ref, err)
	}
	return nil
}
