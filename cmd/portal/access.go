package main

import (
	"context"
	"crypto/hkdf"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
	"log/slog"
	"net/url"
	"strconv"
	"strings"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/settings"
	"github.com/IniZio/moor/internal/wire"
)

// tokens mints the token of each proxy of a task. A token is the proxy's kind and an HMAC of the kind and
// the task under a key drawn from the portal's secret key, so every replica checks it with no table.
type tokens struct{ key []byte }

func newTokens(secret []byte) tokens {
	key, err := hkdf.Key(sha256.New, secret, nil, "moor proxy tokens", 32)
	if err != nil {
		panic(fmt.Sprintf("proxy token key: %v", err)) // only a key length past HKDF's limit fails, and 32 is not
	}
	return tokens{key: key}
}

func (t tokens) sum(task domain.TaskID, kind domain.ProxyKind) []byte {
	mac := hmac.New(sha256.New, t.key)
	mac.Write([]byte(string(kind) + "\x00" + string(task)))
	return mac.Sum(nil)
}

func (t tokens) mint(task domain.TaskID, kind domain.ProxyKind) string {
	return string(kind) + "." + base64.RawURLEncoding.EncodeToString(t.sum(task, kind))
}

// verify says which proxy of task the token belongs to.
func (t tokens) verify(task domain.TaskID, token string) (domain.ProxyKind, bool) {
	kind, sig, _ := strings.Cut(token, ".")
	got, err := base64.RawURLEncoding.DecodeString(sig)
	k := domain.ProxyKind(kind)
	return k, err == nil && k.Valid() && hmac.Equal(got, t.sum(task, k))
}

// access is what task.start tells a task's proxies: the portal's address and their two tokens.
func (t tokens) access(portal string) func(domain.Task) wire.ProxyAccess {
	return func(task domain.Task) wire.ProxyAccess {
		return wire.ProxyAccess{Portal: portal, BoxToken: t.mint(task.ID, domain.BoxProxy), DeskToken: t.mint(task.ID, domain.DeskProxy)}
	}
}

// proxyAPI answers a task's proxies while the task runs. The token says which proxy asks.
type proxyAPI struct {
	tokens tokens
	svc    *settings.Service
	db     *store.Store
	portal domain.Rule
	hub    *githubApp
	log    *slog.Logger
}

func newProxyAPI(cfg config, svc *settings.Service, db *store.Store, hub *githubApp) proxyAPI {
	return proxyAPI{tokens: newTokens(cfg.secretKey), svc: svc, db: db, portal: portalRule(cfg.publicURL), hub: hub, log: slog.Default()}
}

// portalRule lets the desk reach the portal at its public address.
func portalRule(public string) domain.Rule {
	u, err := url.Parse(public)
	if err != nil || u.Hostname() == "" {
		return domain.Rule{}
	}
	port, err := strconv.Atoi(u.Port())
	if err != nil {
		port = 443
		if u.Scheme == "http" {
			port = 80
		}
	}
	return domain.AnyRequest("portal", domain.HostPattern(strings.ToLower(u.Hostname())), port)
}

// kind says which proxy of the running task the token is, and the task's repository.
func (a proxyAPI) kind(ctx context.Context, task domain.TaskID, token string) (domain.ProxyKind, domain.Repo, error) {
	kind, ok := a.tokens.verify(task, token)
	if !ok {
		return "", domain.Repo{}, domain.ErrBadToken
	}
	t, err := a.db.Load(ctx, task)
	switch {
	case errors.Is(err, domain.ErrNotFound):
		return "", domain.Repo{}, domain.ErrBadToken
	case err != nil:
		return "", domain.Repo{}, fmt.Errorf("task %s: %w", task, err)
	case !t.State.Live():
		return "", domain.Repo{}, fmt.Errorf("task %s is %s: %w", task, t.State, domain.ErrBadToken)
	}
	return kind, t.Repo, nil
}

func (a proxyAPI) Policy(ctx context.Context, task domain.TaskID, token string) (domain.Policy, error) {
	kind, repo, err := a.kind(ctx, task, token)
	if err != nil {
		return domain.Policy{}, err
	}
	p, err := a.svc.Policy(ctx, kind, a.portal, repo)
	if err != nil {
		return domain.Policy{}, err //nolint:wrapcheck // the web branches on the service's own errors
	}
	p.DefaultBranch, err = a.defaultBranch(ctx, task, repo)
	return p, err
}

// defaultBranch is the task's repository's default branch, read from GitHub on the task's first policy read and
// kept on the task. A read that fails is kept as unknown, so the push guard refuses main and master instead.
func (a proxyAPI) defaultBranch(ctx context.Context, task domain.TaskID, repo domain.Repo) (string, error) {
	branch, read, err := a.db.DefaultBranch(ctx, task)
	if err != nil || read {
		return branch, err //nolint:wrapcheck // the store names the task
	}
	branch, err = a.hub.DefaultBranch(ctx, repo)
	if err != nil {
		a.log.Warn("default branch", "task", task, "err", err)
	}
	if err := a.db.SetDefaultBranch(ctx, task, branch); err != nil {
		return "", err //nolint:wrapcheck // the store names the task
	}
	branch, _, err = a.db.DefaultBranch(ctx, task)
	return branch, err //nolint:wrapcheck // the store names the task
}

func (a proxyAPI) Value(ctx context.Context, task domain.TaskID, token, credential string) (domain.CredentialValue, error) {
	kind, repo, err := a.kind(ctx, task, token)
	if err != nil {
		return domain.CredentialValue{}, err
	}
	return a.svc.Value(ctx, kind, repo, credential) //nolint:wrapcheck // the web branches on the service's own errors
}

func (a proxyAPI) Blocked(ctx context.Context, task domain.TaskID, token string, h domain.BlockedHit) error {
	kind, _, err := a.kind(ctx, task, token)
	if err != nil {
		return err
	}
	h.Task, h.Proxy = task, kind
	return a.svc.RecordBlocked(ctx, h) //nolint:wrapcheck // the web branches on the service's own errors
}

// ruleEnv reads the variables the newest settings set for a task from each rule the proxy of kind enforces, a
// placeholder as its fake token and {repo} as the task's repository. The Box egress proxy's rules set the Box's, and
// the model rules, which only the desk proxy enforces, set the harness session's. A failed read sets none.
func ruleEnv(svc *settings.Service, kind domain.ProxyKind, log *slog.Logger) func(context.Context, domain.Task) []wire.EnvVar {
	return func(ctx context.Context, t domain.Task) []wire.EnvVar {
		p, err := svc.Policy(ctx, kind, domain.Rule{}, t.Repo)
		if err != nil {
			log.Warn("rule env", "proxy", kind, "task", t.ID, "err", err)
			return nil
		}
		var out []wire.EnvVar
		for _, r := range p.Rules {
			for _, v := range r.BoxEnv {
				out = append(out, wire.EnvVar{Name: v.Name, Value: r.BoxValue(v, t.Repo)})
			}
		}
		return out
	}
}

// AllowEntry lets the settings service add a blocked request's entry through the yamlfile adapter.
func (yamlParser) AllowEntry(text, rule, entry string, host domain.HostPort) (string, int, error) {
	out, line, err := yamlfile.AllowEntry([]byte(text), yamlfile.AllowRequest{Rule: rule, Entry: entry, Host: host})
	if err != nil {
		return "", 0, asProblems(err)
	}
	return string(out), line, nil
}
