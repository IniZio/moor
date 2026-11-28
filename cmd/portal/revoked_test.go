package main

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"net/http"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
)

func TestARevokedMachineIsRefusedWithTheReasonRevokedAndOnlyWhenItProvesItsKey(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	m := enrol(t, p, "host-a")
	p.do(req{method: http.MethodPost, path: "/api/machines/" + m.id + "/revoke", person: "Mia"}).has(t, http.StatusNoContent)
	_, stranger, _ := ed25519.GenerateKey(rand.Reader)
	for _, c := range []struct {
		who    string
		key    ed25519.PrivateKey
		reason string
	}{
		{"the revoked machine's own key", m.key, "revoked"},
		{"another key for the same id", stranger, "unauthorized"},
	} {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		agent := link.AgentConfig{URL: "ws://" + p.addr + "/runner/link", Runner: domain.RunnerID(m.id), Key: c.key, Build: "test"}
		_, _, err := link.Dial(ctx, agent)
		cancel()
		var refused *link.RefusedError
		if !errors.As(err, &refused) || refused.Reason != c.reason {
			t.Errorf("%s: dial gave %v, want a refusal with the reason %q", c.who, err, c.reason)
		}
	}
}
