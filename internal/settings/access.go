package settings

import (
	"context"
	"fmt"
	"slices"

	"github.com/IniZio/moor/internal/domain"
)

// Policy is what the proxy of kind enforces now, read from the newest version. portal is the rule that lets
// the desk reach the portal.
func (s *Service) Policy(ctx context.Context, kind domain.ProxyKind, portal domain.AllowRule) (domain.Policy, error) {
	cur, err := s.Current(ctx)
	if err != nil {
		return domain.Policy{}, err
	}
	parsed, err := s.parser.Parse(cur.Text)
	if err != nil {
		return domain.Policy{}, fmt.Errorf("read settings v%d: %w", cur.N, err)
	}
	return parsed.Settings.PolicyFor(kind, cur.N, portal), nil
}

// Value opens the stored value of the named connection, only when the newest version gives that connection to
// the proxy of kind and its value is set.
func (s *Service) Value(ctx context.Context, kind domain.ProxyKind, name string) (string, error) {
	p, err := s.Policy(ctx, kind, domain.AllowRule{})
	if err != nil {
		return "", err
	}
	i := slices.IndexFunc(p.Connections, func(c domain.Connection) bool { return c.Name == name })
	if i < 0 || p.Connections[i].Secret != domain.SecretSet {
		return "", fmt.Errorf("connection %q on the %s proxy: %w", name, kind, domain.ErrNotFound)
	}
	sealed, err := s.store.Secret(ctx, name)
	if err != nil {
		return "", fmt.Errorf("value of %q: %w", name, err)
	}
	plain, err := s.sealer.Open(sealed.Ciphertext, sealed.KeyID)
	if err != nil {
		return "", fmt.Errorf("open the value of %q: %w", name, err)
	}
	return string(plain), nil
}

// RecordBlocked stores a request a proxy refused, counting a repeat on the row it already has.
func (s *Service) RecordBlocked(ctx context.Context, h domain.BlockedHit) error {
	if err := h.Validate(); err != nil {
		return fmt.Errorf("blocked request report: %w", err)
	}
	h.At = s.clock.Now()
	if err := s.store.RecordBlocked(ctx, h); err != nil {
		return fmt.Errorf("record blocked request: %w", err)
	}
	return nil
}

// BlockedRequests lists the task's blocked requests, oldest first.
func (s *Service) BlockedRequests(ctx context.Context, task domain.TaskID) ([]domain.BlockedRequest, error) {
	out, err := s.store.BlockedRequests(ctx, task)
	if err != nil {
		return nil, fmt.Errorf("blocked requests of %s: %w", task, err)
	}
	return out, nil
}

// Allow appends the blocked request's rule to the allow list as a new version, noting the request, and marks
// it allowed. A line of the same text already there is answered instead of a new one.
func (s *Service) Allow(ctx context.Context, task domain.TaskID, id domain.BlockedID, by string) (domain.AllowedLine, error) {
	b, err := s.blocked(ctx, task, id)
	if err != nil {
		return domain.AllowedLine{}, err
	}
	if !b.Allowable() {
		return domain.AllowedLine{}, fmt.Errorf("blocked request %s on the %s proxy: %w", b.Request(), b.Proxy, domain.ErrRefused)
	}
	cur, err := s.Current(ctx)
	if err != nil {
		return domain.AllowedLine{}, err
	}
	text, line, err := s.parser.AppendAllow(cur.Text, b.Rule().String())
	if err != nil {
		return domain.AllowedLine{}, fmt.Errorf("append %s: %w", b.Request(), err)
	}
	note := fmt.Sprintf("Allow on blocked request %s of task #%s", b.Request(), task)
	saved, err := s.Save(ctx, domain.SettingsSave{Base: cur.N, Text: text, Note: note, By: by})
	if err != nil {
		return domain.AllowedLine{}, err
	}
	if err := s.decide(ctx, id, domain.Allowed, by); err != nil {
		return domain.AllowedLine{}, err
	}
	return domain.AllowedLine{Line: line, Version: saved.Version.N}, nil
}

// Dismiss keeps the blocked request on the task as dismissed.
func (s *Service) Dismiss(ctx context.Context, task domain.TaskID, id domain.BlockedID, by string) error {
	if _, err := s.blocked(ctx, task, id); err != nil {
		return err
	}
	return s.decide(ctx, id, domain.Dismissed, by)
}

func (s *Service) decide(ctx context.Context, id domain.BlockedID, d domain.Decision, by string) error {
	if err := s.store.DecideBlocked(ctx, id, d, by, s.clock.Now()); err != nil {
		return fmt.Errorf("decide blocked request %d: %w", id, err)
	}
	return nil
}

// blocked is the row of id, when it is on task.
func (s *Service) blocked(ctx context.Context, task domain.TaskID, id domain.BlockedID) (domain.BlockedRequest, error) {
	b, err := s.store.BlockedRequest(ctx, id)
	if err == nil && b.Task != task {
		err = domain.ErrNotFound
	}
	if err != nil {
		return domain.BlockedRequest{}, fmt.Errorf("blocked request %d of task %s: %w", id, task, err)
	}
	return b, nil
}
