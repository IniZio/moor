// Package settingstest holds an in-memory settings store that keeps to the same contract as the database one.
package settingstest

import (
	"context"
	"fmt"
	"slices"
	"sync"

	"github.com/IniZio/moor/internal/domain"
)

type Memory struct {
	mu       sync.Mutex
	versions []domain.SettingsVersion
	secrets  map[string]domain.SealedSecret
}

func NewMemory() *Memory { return &Memory{secrets: map[string]domain.SealedSecret{}} }

func (m *Memory) LatestSettings(context.Context) (domain.SettingsVersion, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if len(m.versions) == 0 {
		return domain.SettingsVersion{}, fmt.Errorf("settings: %w", domain.ErrNotFound)
	}
	return m.versions[len(m.versions)-1], nil
}

func (m *Memory) SettingsVersions(context.Context) ([]domain.SettingsVersion, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := slices.Clone(m.versions)
	slices.Reverse(out)
	return out, nil
}

func (m *Memory) SaveSettings(_ context.Context, w domain.SettingsWrite) (domain.SettingsVersion, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if w.Base != len(m.versions) {
		return domain.SettingsVersion{}, fmt.Errorf("base %d, newest %d: %w", w.Base, len(m.versions), domain.ErrStaleSettings)
	}
	v := domain.SettingsVersion{N: w.Base + 1, Text: w.Text, Note: w.Note, SavedBy: w.By, SavedAt: w.At.UTC()}
	m.versions = append(m.versions, v)
	for name := range m.secrets {
		if !slices.Contains(w.Keep, name) {
			delete(m.secrets, name)
		}
	}
	for _, s := range w.Secrets {
		s.Ciphertext = slices.Clone(s.Ciphertext)
		s.UpdatedAt = s.UpdatedAt.UTC()
		m.secrets[s.Connection] = s
	}
	return v, nil
}

func (m *Memory) Secret(_ context.Context, connection string) (domain.SealedSecret, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	s, ok := m.secrets[connection]
	if !ok {
		return domain.SealedSecret{}, fmt.Errorf("secret of %q: %w", connection, domain.ErrNotFound)
	}
	s.Ciphertext = slices.Clone(s.Ciphertext)
	return s, nil
}
