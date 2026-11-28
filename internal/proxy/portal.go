package proxy

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
	"unicode"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// Run reads the task's policy now and then every Poll until ctx ends. A failed read keeps the policy the
// proxy last read.
func (p *Proxy) Run(ctx context.Context) {
	tick := time.NewTicker(p.cfg.Poll)
	defer tick.Stop()
	for {
		if err := p.Refresh(ctx); err != nil {
			p.cfg.Log.Warn("policy", "err", err)
		}
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
	}
}

// errBadToken is the portal refusing the proxy's token, as it does once the task stops running.
var errBadToken = errors.New("the portal refused the proxy's token")

// Refresh reads the task's policy once. A refused token drops the policy, so the proxy refuses everything.
func (p *Proxy) Refresh(ctx context.Context) error {
	var w wire.Policy
	if err := p.call(ctx, http.MethodGet, "/internal/policy/"+url.PathEscape(p.cfg.Task), nil, &w); err != nil {
		if errors.Is(err, errBadToken) {
			p.policy.Store(nil)
		}
		return err
	}
	pol, err := policyOf(w)
	if err != nil {
		return err
	}
	p.keepNewest(&pol)
	return nil
}

// keepNewest stores pol unless the proxy holds a newer version: a read that began before another one and landed
// after it must not put an older policy back.
func (p *Proxy) keepNewest(pol *domain.Policy) {
	for {
		old := p.policy.Load()
		if old != nil && old.Version > pol.Version {
			return
		}
		if p.policy.CompareAndSwap(old, pol) {
			if old == nil || old.Version != pol.Version {
				p.cfg.Log.Info("policy", "version", pol.Version, "rules", len(pol.Rules))
			}
			return
		}
	}
}

// policyOf reads the portal's answer with the settings document's own parsers.
func policyOf(w wire.Policy) (domain.Policy, error) {
	p := domain.Policy{
		Version: w.Version, Repo: domain.Repo{Host: domain.RepoHost(w.Repo.Host), Path: domain.RepoPath(w.Repo.Path)},
		Set: map[string]bool{}, DefaultBranch: w.DefaultBranch,
	}
	for _, wr := range w.Rules {
		r, err := ruleOf(wr)
		if err != nil {
			return domain.Policy{}, fmt.Errorf("policy v%d rule %q: %w", w.Version, wr.Name, err)
		}
		p.Rules = append(p.Rules, r)
		if wr.Set && wr.Credential != "" {
			p.Set[wr.Credential] = true
		}
	}
	return p, nil
}

func ruleOf(w wire.PolicyRule) (domain.Rule, error) {
	r := domain.Rule{Name: w.Name, Credential: w.Credential, GraphQLMutations: w.GraphQLMutations}
	for _, h := range w.Hosts {
		hp, err := domain.ParseHostPort(h)
		if err != nil {
			return domain.Rule{}, fmt.Errorf("host: %w", err)
		}
		r.Hosts = append(r.Hosts, hp)
	}
	var err error
	if len(w.Allow) == 1 && w.Allow[0] == domain.AnyText {
		r.Allow.Any = true
	} else if r.Allow.Entries, err = entriesOf(w.Allow); err != nil {
		return domain.Rule{}, err
	}
	if r.Ask, err = entriesOf(w.Ask); err != nil {
		return domain.Rule{}, err
	}
	if w.Inject != nil && w.Inject.BasicUser != "" {
		r.Inject = domain.BasicInject{User: w.Inject.BasicUser}
	} else if w.Inject != nil {
		r.Inject = domain.HeaderInject{Name: w.Inject.Header, Form: w.Inject.Form}
	}
	return r, nil
}

func entriesOf(lines []string) ([]domain.Entry, error) {
	out := make([]domain.Entry, 0, len(lines))
	for _, l := range lines {
		e, err := domain.ParseEntry(l)
		if err != nil {
			return nil, fmt.Errorf("entry: %w", err)
		}
		out = append(out, e)
	}
	return out, nil
}

// value is what the credential adds under policy version, read from the portal when the proxy holds none read
// under that version, or holds a token about to expire.
func (p *Proxy) value(ctx context.Context, version int, name string) (string, error) {
	p.mu.Lock()
	c, ok := p.values[name]
	p.mu.Unlock()
	fresh := c.value.Expires.IsZero() || time.Until(c.value.Expires) > time.Minute
	if ok && c.version == version && fresh {
		return c.value.Value, nil
	}
	var out wire.ValueReply
	if err := p.call(ctx, http.MethodPost, "/internal/credentials/value", wire.ValueRequest{Task: p.cfg.Task, Credential: name}, &out); err != nil {
		return "", err
	}
	p.mu.Lock()
	p.values[name] = cached{version: version, value: domain.CredentialValue{Value: out.Value, Expires: out.Expires}}
	p.mu.Unlock()
	return out.Value, nil
}

// report tells the portal about a refused request. It runs beside the refusal, which never waits for it. The
// path goes without its query, and the detail names GraphQL fields at most.
func (p *Proxy) report(host string, port int, method domain.Method, path string, v verdict) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	in := wire.BlockedReport{
		Task: p.cfg.Task, Host: host, Port: port, Method: string(method), Path: reportPath(path),
		Rule: v.rule, Entry: v.entry, Reason: string(v.reason), Detail: v.detail,
	}
	if err := p.call(ctx, http.MethodPost, "/internal/blocked", in, nil); err != nil {
		p.cfg.Log.Warn("report blocked request", "host", host, "err", err)
	}
}

// reportPath is the path as a blocked request keeps it: at most 512 bytes, with no control character.
func reportPath(path string) string {
	if len(path) > 512 || strings.ContainsFunc(path, unicode.IsControl) {
		return ""
	}
	return path
}

func (p *Proxy) call(ctx context.Context, method, path string, body, out any) error {
	var r io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("portal %s: %w", path, err)
		}
		r = bytes.NewReader(b)
	}
	req, err := http.NewRequestWithContext(ctx, method, p.cfg.Portal+path, r)
	if err != nil {
		return fmt.Errorf("portal %s: %w", path, err)
	}
	req.Header.Set("Authorization", "Bearer "+p.cfg.Token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := p.cfg.Client.Do(req)
	if err != nil {
		return fmt.Errorf("portal %s: %w", path, err)
	}
	defer func() { _ = resp.Body.Close() }()
	switch {
	case resp.StatusCode == http.StatusUnauthorized:
		return fmt.Errorf("portal %s: %w", path, errBadToken)
	case resp.StatusCode >= 300:
		return fmt.Errorf("portal %s: %s", path, resp.Status)
	}
	if out == nil {
		return nil
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(out); err != nil {
		return fmt.Errorf("portal %s: %w", path, err)
	}
	return nil
}
