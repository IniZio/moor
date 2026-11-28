package proxy_test

import (
	"context"
	"net/http"
	"testing"

	"github.com/IniZio/moor/internal/wire"
)

// A read that began before a newer one can land after it. The proxy keeps enforcing the newer policy.
func TestAnOlderPolicyReadNeverReplacesANewerOne(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(policy(2, rule("staging", up, "GET /**")))
	waitFor(t, "version 2", func() bool { return g.p.Refresh(context.Background()) == nil })
	g.portal.set(policy(1))
	if err := g.p.Refresh(context.Background()); err != nil {
		t.Fatal(err)
	}
	if resp, body := do(t, g.client, "GET", "https://"+up+"/x", nil); resp.StatusCode != http.StatusOK {
		t.Fatalf("after an older read: %s: %s", resp.Status, body)
	}
}
