package store

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"

	"github.com/IniZio/moor/internal/domain"
)

const selectSettings = "SELECT version, text, note, saved_by, saved_at FROM settings_documents "

func scanSettings(row pgx.Row) (domain.SettingsVersion, error) {
	var v domain.SettingsVersion
	if err := row.Scan(&v.N, &v.Text, &v.Note, &v.SavedBy, &v.SavedAt); err != nil {
		return domain.SettingsVersion{}, fmt.Errorf("read settings version: %w", err)
	}
	v.SavedAt = v.SavedAt.UTC()
	return v, nil
}

func (s *Store) LatestSettings(ctx context.Context) (domain.SettingsVersion, error) {
	v, err := scanSettings(s.db.QueryRow(ctx, selectSettings+"ORDER BY version DESC LIMIT 1"))
	if errors.Is(err, pgx.ErrNoRows) {
		return domain.SettingsVersion{}, fmt.Errorf("settings: %w", domain.ErrNotFound)
	}
	return v, err
}

func (s *Store) SettingsVersions(ctx context.Context) ([]domain.SettingsVersion, error) {
	rows, err := s.db.Query(ctx, selectSettings+"ORDER BY version DESC")
	if err != nil {
		return nil, fmt.Errorf("settings versions: %w", err)
	}
	defer rows.Close()
	var out []domain.SettingsVersion
	for rows.Next() {
		v, err := scanSettings(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("settings versions: %w", err)
	}
	return out, nil
}

// SaveSettings writes the version and its secrets in one transaction. Two saves on one base both try to
// insert the same version number, so the primary key lets one win and turns the other into ErrStaleSettings.
func (s *Store) SaveSettings(ctx context.Context, w domain.SettingsWrite) (domain.SettingsVersion, error) {
	var saved domain.SettingsVersion
	err := pgx.BeginFunc(ctx, s.db, func(tx pgx.Tx) error {
		var newest int
		if err := tx.QueryRow(ctx, "SELECT coalesce(max(version), 0) FROM settings_documents").Scan(&newest); err != nil {
			return fmt.Errorf("newest settings version: %w", err)
		}
		if newest != w.Base {
			return fmt.Errorf("base %d, newest %d: %w", w.Base, newest, domain.ErrStaleSettings)
		}
		saved = domain.SettingsVersion{N: w.Base + 1, Text: w.Text, Note: w.Note, SavedBy: w.By, SavedAt: w.At.UTC()}
		_, err := tx.Exec(ctx, "INSERT INTO settings_documents (version, text, note, saved_by, saved_at) VALUES ($1, $2, $3, $4, $5)",
			saved.N, saved.Text, saved.Note, saved.SavedBy, saved.SavedAt)
		if err != nil {
			return unique(err, "settings version", domain.ErrStaleSettings)
		}
		return keepSecrets(ctx, tx, w)
	})
	if err != nil {
		return domain.SettingsVersion{}, fmt.Errorf("save settings: %w", err)
	}
	return saved, nil
}

func keepSecrets(ctx context.Context, tx pgx.Tx, w domain.SettingsWrite) error {
	keep := w.Keep
	if keep == nil {
		keep = []string{}
	}
	if _, err := tx.Exec(ctx, "DELETE FROM secret_values WHERE NOT (connection = ANY($1))", keep); err != nil {
		return fmt.Errorf("drop secrets of removed connections: %w", err)
	}
	for _, sec := range w.Secrets {
		_, err := tx.Exec(ctx, `INSERT INTO secret_values (connection, ciphertext, key_id, updated_at) VALUES ($1, $2, $3, $4)
			ON CONFLICT (connection) DO UPDATE SET ciphertext = $2, key_id = $3, updated_at = $4`,
			sec.Connection, sec.Ciphertext, sec.KeyID, sec.UpdatedAt.UTC())
		if err != nil {
			return fmt.Errorf("store secret of %q: %w", sec.Connection, err)
		}
	}
	return nil
}

func (s *Store) Secret(ctx context.Context, connection string) (domain.SealedSecret, error) {
	sec := domain.SealedSecret{Connection: connection}
	err := s.db.QueryRow(ctx, "SELECT ciphertext, key_id, updated_at FROM secret_values WHERE connection = $1", connection).
		Scan(&sec.Ciphertext, &sec.KeyID, &sec.UpdatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return domain.SealedSecret{}, fmt.Errorf("secret of %q: %w", connection, domain.ErrNotFound)
	}
	if err != nil {
		return domain.SealedSecret{}, fmt.Errorf("secret of %q: %w", connection, err)
	}
	sec.UpdatedAt = sec.UpdatedAt.UTC()
	return sec, nil
}
