package settings

import (
	"context"
	"errors"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
)

// ErrNoApp is a call that needs the GitHub App when the settings hold none with its private key set.
var ErrNoApp = errors.New("the settings hold no GitHub App with its private key set")

// current is the newest version, parsed.
func (s *Service) current(ctx context.Context) (domain.SettingsVersion, Parsed, error) {
	cur, err := s.Current(ctx)
	if err != nil {
		return cur, Parsed{}, err
	}
	parsed, err := s.parser.Parse(cur.Text)
	if err != nil {
		return cur, Parsed{}, fmt.Errorf("read settings v%d: %w", cur.N, err)
	}
	return cur, parsed, nil
}

// Policy is what the proxy of kind enforces now for a task on repo, read from the newest version. portal is the
// rule that lets the desk reach the portal.
func (s *Service) Policy(ctx context.Context, kind domain.ProxyKind, portal domain.Rule, repo domain.Repo) (domain.Policy, error) {
	cur, parsed, err := s.current(ctx)
	if err != nil {
		return domain.Policy{}, err
	}
	return parsed.Settings.PolicyFor(kind, cur.N, portal, repo), nil
}

// Value is what the named credential adds for a task on repo, only when the newest version gives it to the proxy
// of kind: its stored value, or for a GitHub App a token minted for the task's repository.
func (s *Service) Value(ctx context.Context, kind domain.ProxyKind, repo domain.Repo, name string) (domain.CredentialValue, error) {
	cur, parsed, err := s.current(ctx)
	if err != nil {
		return domain.CredentialValue{}, err
	}
	if !parsed.Settings.PolicyFor(kind, cur.N, domain.Rule{}, repo).Gives(name) {
		return domain.CredentialValue{}, fmt.Errorf("credential %q on the %s proxy: %w", name, kind, domain.ErrNotFound)
	}
	c, _ := parsed.Settings.Credential(name)
	if c.App == nil {
		v, err := s.open(ctx, domain.SecretKey(name, domain.FieldValue))
		return domain.CredentialValue{Value: string(v)}, err
	}
	keys, err := s.appKeys(ctx, c)
	if err != nil {
		return domain.CredentialValue{}, err
	}
	v, err := s.minter.Mint(ctx, keys, repo)
	if err != nil {
		return domain.CredentialValue{}, fmt.Errorf("mint a token of %q for %s: %w", name, repo, err)
	}
	return v, nil
}

// GitHubApp is the newest version's GitHub App with its secrets open, for the hook and the answers.
func (s *Service) GitHubApp(ctx context.Context) (domain.AppKeys, error) {
	_, parsed, err := s.current(ctx)
	if err != nil {
		return domain.AppKeys{}, err
	}
	c, ok := parsed.Settings.GitHubApp()
	if !ok || !c.Set() {
		return domain.AppKeys{}, ErrNoApp
	}
	return s.appKeys(ctx, c)
}

func (s *Service) appKeys(ctx context.Context, c domain.Credential) (domain.AppKeys, error) {
	keys := domain.AppKeys{AppID: c.App.AppID, APIURL: c.App.APIURL}
	var err error
	if keys.PrivateKey, err = s.open(ctx, domain.SecretKey(c.Name, domain.FieldPrivateKey)); err != nil {
		return domain.AppKeys{}, err
	}
	if c.App.WebhookSecret == domain.SecretSet {
		keys.WebhookSecret, err = s.open(ctx, domain.SecretKey(c.Name, domain.FieldWebhookSecret))
	}
	return keys, err
}

func (s *Service) open(ctx context.Context, key string) ([]byte, error) {
	sealed, err := s.store.Secret(ctx, key)
	if err != nil {
		return nil, fmt.Errorf("secret %q: %w", key, err)
	}
	plain, err := s.sealer.Open(sealed.Ciphertext, sealed.KeyID)
	if err != nil {
		return nil, fmt.Errorf("open the secret %q: %w", key, err)
	}
	return plain, nil
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

// Allow adds the blocked request's entry to its rule, or a rule for its host, as a new version noting the
// request, and marks it allowed. An entry already there is answered instead of a new one.
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
	text, line, err := s.parser.AllowEntry(cur.Text, b.Rule, b.Entry, domain.HostPort{Host: domain.HostPattern(b.Host), Port: b.Port})
	if err != nil {
		return domain.AllowedLine{}, fmt.Errorf("allow %s: %w", b.Request(), err)
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
