package settings

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// InitialText is what the editor shows until the first save.

type Service struct {
	store  Store
	parser Parser
	sealer Sealer
	clock  Clock
	minter Minter
}

func New(store Store, parser Parser, sealer Sealer, clock Clock, minter Minter) *Service {
	return &Service{store: store, parser: parser, sealer: sealer, clock: clock, minter: minter}
}

// Current is the newest version, or version 0 with InitialText before the first save.
func (s *Service) Current(ctx context.Context) (domain.SettingsVersion, error) {
	v, err := s.store.LatestSettings(ctx)
	if errors.Is(err, domain.ErrNotFound) {
		return domain.SettingsVersion{Text: InitialText}, nil
	}
	if err != nil {
		return domain.SettingsVersion{}, fmt.Errorf("load settings: %w", err)
	}
	return v, nil
}

// Allowed lists the repositories the newest version allows.
func (s *Service) Allowed(ctx context.Context) ([]domain.Repo, error) {
	cur, err := s.Current(ctx)
	if err != nil {
		return nil, err
	}
	parsed, err := s.parser.Parse(cur.Text)
	if err != nil {
		return nil, fmt.Errorf("read settings v%d: %w", cur.N, err)
	}
	return parsed.Repositories, nil
}

// Check reports the problems of a document without saving it.
func (s *Service) Check(text string) []domain.SettingsProblem {
	_, err := s.parser.Parse(text)
	var invalid *domain.InvalidSettingsError
	if errors.As(err, &invalid) {
		return invalid.Problems
	}
	return nil
}

// Save stores the document as a new version. Typed secret values are sealed and stored apart from it.
func (s *Service) Save(ctx context.Context, in domain.SettingsSave) (domain.SettingsSaved, error) {
	cur, err := s.Current(ctx)
	if err != nil {
		return domain.SettingsSaved{}, err
	}
	if cur.N != in.Base {
		return domain.SettingsSaved{}, &domain.StaleSettingsError{Newest: cur.N}
	}
	parsed, err := s.parser.Parse(in.Text)
	if err != nil {
		return domain.SettingsSaved{}, fmt.Errorf("check settings: %w", err)
	}
	if cur.N > 0 && parsed.Text == cur.Text && len(parsed.Secrets) == 0 {
		return domain.SettingsSaved{Version: cur, Unchanged: true}, nil
	}
	sealed, err := s.seal(parsed.Secrets)
	if err != nil {
		return domain.SettingsSaved{}, err
	}
	replaced := make([]string, len(parsed.Secrets))
	for i, sec := range parsed.Secrets {
		replaced[i] = sec.Key
	}
	note := in.Note
	if note == "" && len(replaced) > 0 {
		note = "replaced the value of " + strings.Join(replaced, ", ")
	}
	v, err := s.store.SaveSettings(ctx, domain.SettingsWrite{
		Base: in.Base, Text: parsed.Text, Note: note, By: in.By, At: s.clock.Now(), Keep: parsed.Settings.SecretKeys(), Secrets: sealed,
	})
	if err != nil {
		return domain.SettingsSaved{}, s.staleOrErr(ctx, err)
	}
	return domain.SettingsSaved{Version: v, Replaced: replaced}, nil
}

func (s *Service) seal(secrets []domain.NewSecret) ([]domain.SealedSecret, error) {
	now := s.clock.Now()
	out := make([]domain.SealedSecret, 0, len(secrets))
	for _, sec := range secrets {
		ciphertext, keyID, err := s.sealer.Seal([]byte(sec.Value))
		if err != nil {
			return nil, fmt.Errorf("seal the secret %q: %w", sec.Key, err)
		}
		out = append(out, domain.SealedSecret{Key: sec.Key, Ciphertext: ciphertext, KeyID: keyID, UpdatedAt: now})
	}
	return out, nil
}

// staleOrErr names the newest version when the store refused a stale base, so the person can reload.
func (s *Service) staleOrErr(ctx context.Context, err error) error {
	if !errors.Is(err, domain.ErrStaleSettings) {
		return err
	}
	cur, curErr := s.Current(ctx)
	if curErr != nil {
		return err
	}
	return &domain.StaleSettingsError{Newest: cur.N}
}
