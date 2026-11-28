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
	"time"

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
	if old := p.policy.Swap(&pol); old == nil || old.Version != pol.Version {
		p.cfg.Log.Info("policy", "version", pol.Version, "allow", len(pol.Allow), "connections", len(pol.Connections))
	}
	return nil
}

// policyOf reads the portal's answer with the settings document's own parsers.
func policyOf(w wire.Policy) (domain.Policy, error) {
	p := domain.Policy{Version: w.Version}
	for _, line := range w.Allow {
		r, err := domain.ParseAllowRule(line)
		if err != nil {
			return domain.Policy{}, fmt.Errorf("policy v%d: %w", w.Version, err)
		}
		p.Allow = append(p.Allow, r)
	}
	for _, c := range w.Connections {
		conn, err := connectionOf(c)
		if err != nil {
			return domain.Policy{}, fmt.Errorf("policy v%d: %w", w.Version, err)
		}
		p.Connections = append(p.Connections, conn)
	}
	return p, nil
}

func connectionOf(c wire.PolicyConnection) (domain.Connection, error) {
	add, err := domain.ParseAddRule(c.Add)
	if err != nil {
		return domain.Connection{}, fmt.Errorf("connection %q: %w", c.Name, err)
	}
	conn := domain.Connection{Name: c.Name, Add: add, Secret: domain.SecretNotSet}
	if c.Set {
		conn.Secret = domain.SecretSet
	}
	for _, h := range c.Hosts {
		hp, err := domain.ParseHostPattern(h)
		if err != nil {
			return domain.Connection{}, fmt.Errorf("connection %q: %w", c.Name, err)
		}
		conn.Hosts = append(conn.Hosts, hp)
	}
	return conn, nil
}

// value is the connection's value under policy version, read from the portal when the proxy holds none
// read under that version.
func (p *Proxy) value(ctx context.Context, version int, name string) (string, error) {
	p.mu.Lock()
	c, ok := p.values[name]
	p.mu.Unlock()
	if ok && c.version == version {
		return c.value, nil
	}
	var out wire.ValueReply
	if err := p.call(ctx, http.MethodPost, "/internal/connections/value", wire.ValueRequest{Task: p.cfg.Task, Connection: name}, &out); err != nil {
		return "", err
	}
	p.mu.Lock()
	p.values[name] = cached{version: version, value: out.Value}
	p.mu.Unlock()
	return out.Value, nil
}

// report tells the portal about a refused request. It runs beside the refusal, which never waits for it.
func (p *Proxy) report(host string, port int, method domain.Method, reason domain.DenyReason) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	in := wire.BlockedReport{Task: p.cfg.Task, Host: host, Port: port, Method: string(method), Reason: string(reason)}
	if err := p.call(ctx, http.MethodPost, "/internal/blocked", in, nil); err != nil {
		p.cfg.Log.Warn("report blocked request", "host", host, "err", err)
	}
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
