package main

import (
	"context"
	"crypto/hkdf"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
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
	portal domain.AllowRule
}

func newProxyAPI(cfg config, svc *settings.Service, db *store.Store) proxyAPI {
	return proxyAPI{tokens: newTokens(cfg.secretKey), svc: svc, db: db, portal: portalRule(cfg.publicURL)}
}

// portalRule lets the desk reach the portal at its public address.
func portalRule(public string) domain.AllowRule {
	u, err := url.Parse(public)
	if err != nil || u.Hostname() == "" {
		return domain.AllowRule{}
	}
	port, err := strconv.Atoi(u.Port())
	if err != nil {
		port = 443
		if u.Scheme == "http" {
			port = 80
		}
	}
	return domain.AnyMethod(domain.HostPattern(strings.ToLower(u.Hostname())), port)
}

func (a proxyAPI) kind(ctx context.Context, task domain.TaskID, token string) (domain.ProxyKind, error) {
	kind, ok := a.tokens.verify(task, token)
	if !ok {
		return "", domain.ErrBadToken
	}
	t, err := a.db.Load(ctx, task)
	switch {
	case errors.Is(err, domain.ErrNotFound):
		return "", domain.ErrBadToken
	case err != nil:
		return "", fmt.Errorf("task %s: %w", task, err)
	case !t.State.Live():
		return "", fmt.Errorf("task %s is %s: %w", task, t.State, domain.ErrBadToken)
	}
	return kind, nil
}

func (a proxyAPI) Policy(ctx context.Context, task domain.TaskID, token string) (domain.Policy, error) {
	kind, err := a.kind(ctx, task, token)
	if err != nil {
		return domain.Policy{}, err
	}
	return a.svc.Policy(ctx, kind, a.portal) //nolint:wrapcheck // the web branches on the service's own errors
}

func (a proxyAPI) Value(ctx context.Context, task domain.TaskID, token, connection string) (string, error) {
	kind, err := a.kind(ctx, task, token)
	if err != nil {
		return "", err
	}
	return a.svc.Value(ctx, kind, connection) //nolint:wrapcheck // the web branches on the service's own errors
}

func (a proxyAPI) Blocked(ctx context.Context, task domain.TaskID, token string, h domain.BlockedHit) error {
	kind, err := a.kind(ctx, task, token)
	if err != nil {
		return err
	}
	h.Task, h.Proxy = task, kind
	return a.svc.RecordBlocked(ctx, h) //nolint:wrapcheck // the web branches on the service's own errors
}

// AppendAllow lets the settings service add an allow line through the yamlfile adapter.
func (yamlParser) AppendAllow(text, rule string) (string, int, error) {
	out, line, err := yamlfile.AppendAllow([]byte(text), rule)
	if err != nil {
		return "", 0, asProblems(err)
	}
	return string(out), line, nil
}
