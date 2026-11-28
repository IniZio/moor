package web

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// ProxyAPI answers a task's proxies. The token names the task and the proxy, and each call with a token
// that is not the task's, or of a task that no longer runs, fails with domain.ErrBadToken.
type ProxyAPI interface {
	Policy(ctx context.Context, task domain.TaskID, token string) (domain.Policy, error)
	Value(ctx context.Context, task domain.TaskID, token, connection string) (string, error)
	Blocked(ctx context.Context, task domain.TaskID, token string, h domain.BlockedHit) error
}

const maxProxyBody = 4 << 10

// proxyRoutes are the routes a task's proxies call with their token. They sit outside the person and
// cross-origin checks: no browser calls them.
func (s *server) proxyRoutes(r chi.Router) {
	if s.deps.Proxies == nil {
		return
	}
	r.Get("/internal/policy/{task}", s.proxyPolicy)
	r.Post("/internal/connections/value", s.proxyValue)
	r.Post("/internal/blocked", s.proxyBlocked)
}

func bearer(r *http.Request) string {
	return strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
}

func proxyError(w http.ResponseWriter, r *http.Request, err error) {
	switch {
	case errors.Is(err, domain.ErrBadToken):
		http.Error(w, "not a proxy of this task", http.StatusUnauthorized)
	case errors.Is(err, domain.ErrNotFound):
		http.Error(w, "no such connection for this proxy", http.StatusNotFound)
	case errors.Is(err, domain.ErrBadSettings):
		http.Error(w, "the report is not a blocked request", http.StatusBadRequest)
	default:
		serverError(w, r, err)
	}
}

func (s *server) proxyPolicy(w http.ResponseWriter, r *http.Request) {
	p, err := s.deps.Proxies.Policy(r.Context(), domain.TaskID(chi.URLParam(r, "task")), bearer(r))
	if err != nil {
		proxyError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, policyOf(p))
}

func policyOf(p domain.Policy) wire.Policy {
	out := wire.Policy{Version: p.Version, Allow: make([]string, len(p.Allow)), Connections: make([]wire.PolicyConnection, len(p.Connections))}
	for i, a := range p.Allow {
		out.Allow[i] = a.String()
	}
	for i, c := range p.Connections {
		hosts := make([]string, len(c.Hosts))
		for j, h := range c.Hosts {
			hosts[j] = string(h)
		}
		out.Connections[i] = wire.PolicyConnection{Name: c.Name, Hosts: hosts, Add: c.Add.String(), Set: c.Secret == domain.SecretSet}
	}
	return out
}

func decodeProxy(w http.ResponseWriter, r *http.Request, v any) bool {
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxProxyBody))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		http.Error(w, "the body is not what this route takes", http.StatusBadRequest)
		return false
	}
	return true
}

func (s *server) proxyValue(w http.ResponseWriter, r *http.Request) {
	var in wire.ValueRequest
	if !decodeProxy(w, r, &in) {
		return
	}
	v, err := s.deps.Proxies.Value(r.Context(), domain.TaskID(in.Task), bearer(r), in.Connection)
	if err != nil {
		proxyError(w, r, err)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusOK, wire.ValueReply{Value: v})
}

func (s *server) proxyBlocked(w http.ResponseWriter, r *http.Request) {
	var in wire.BlockedReport
	if !decodeProxy(w, r, &in) {
		return
	}
	h := domain.BlockedHit{Host: in.Host, Port: in.Port, Method: domain.Method(in.Method), Reason: domain.DenyReason(in.Reason)}
	if err := s.deps.Proxies.Blocked(r.Context(), domain.TaskID(in.Task), bearer(r), h); err != nil {
		proxyError(w, r, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
