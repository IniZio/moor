// Package settingstest holds an in-memory settings store that keeps to the same contract as the database one.
package settingstest

import (
	"context"
	"fmt"
	"slices"
	"sync"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

type Memory struct {
	mu       sync.Mutex
	versions []domain.SettingsVersion
	secrets  map[string]domain.SealedSecret
	blocked  []domain.BlockedRequest
}

func NewMemory() *Memory { return &Memory{secrets: map[string]domain.SealedSecret{}} }

func (m *Memory) RecordBlocked(_ context.Context, h domain.BlockedHit) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	for i, b := range m.blocked {
		same := b.Task == h.Task && b.Proxy == h.Proxy && b.Host == h.Host && b.Port == h.Port && b.Method == h.Method
		if same && b.Path == h.Path && b.Detail == h.Detail {
			m.blocked[i].Count++
			m.blocked[i].LastAt, m.blocked[i].Reason, m.blocked[i].Rule, m.blocked[i].Entry = h.At.UTC(), h.Reason, h.Rule, h.Entry
			return nil
		}
	}
	m.blocked = append(m.blocked, domain.BlockedRequest{
		ID: domain.BlockedID(len(m.blocked) + 1), Task: h.Task, Proxy: h.Proxy, Host: h.Host, Port: h.Port, Method: h.Method,
		Path: h.Path, Rule: h.Rule, Entry: h.Entry, Reason: h.Reason, Detail: h.Detail, Count: 1, FirstAt: h.At.UTC(), LastAt: h.At.UTC(),
	})
	return nil
}

func (m *Memory) BlockedRequests(_ context.Context, task domain.TaskID) ([]domain.BlockedRequest, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var out []domain.BlockedRequest
	for _, b := range m.blocked {
		if b.Task == task {
			out = append(out, b)
		}
	}
	return out, nil
}

func (m *Memory) BlockedRequest(_ context.Context, id domain.BlockedID) (domain.BlockedRequest, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if id < 1 || int(id) > len(m.blocked) {
		return domain.BlockedRequest{}, fmt.Errorf("blocked request %d: %w", id, domain.ErrNotFound)
	}
	return m.blocked[id-1], nil
}

func (m *Memory) DecideBlocked(_ context.Context, id domain.BlockedID, d domain.Decision, by string, at time.Time) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if id < 1 || int(id) > len(m.blocked) {
		return fmt.Errorf("blocked request %d: %w", id, domain.ErrNotFound)
	}
	b := &m.blocked[id-1]
	b.Decision, b.DecidedBy, b.DecidedAt = d, by, at.UTC()
	return nil
}

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
		m.secrets[s.Key] = s
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
