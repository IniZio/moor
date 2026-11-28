package proxy_test

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/wire"
)

// placeholder is the fake token the desk holds for the rule named model.
const placeholder = "moor-placeholder-model"

// TestAModelCallGetsTheCredentialInPlaceOfThePlaceholderInBothForms sends what a harness on the desk sends, a
// placeholder where its key or its bearer token goes and the vendor's own headers beside it, to a fake model API.
func TestAModelCallGetsTheCredentialInPlaceOfThePlaceholderInBothForms(t *testing.T) {
	forms := []struct {
		name   string
		inject wire.Inject
		sent   http.Header
		header string
		want   string
	}{
		{"an API key", wire.Inject{Header: "X-Api-Key", Form: "{secret}"}, http.Header{"X-Api-Key": {placeholder}}, "X-Api-Key", value},
		{"a bearer token", wire.Inject{Header: "Authorization", Form: "Bearer {secret}"}, http.Header{"Authorization": {"Bearer " + placeholder}}, "Authorization", "Bearer " + value},
	}
	for _, f := range forms {
		t.Run(f.name, func(t *testing.T) { modelCall(t, f.inject, f.sent, f.header, f.want) })
	}
}

func modelCall(t *testing.T, inject wire.Inject, sent http.Header, header, want string) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(policy(1, withCred(rule("model", up, "*"), "model", inject)))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	sent = sent.Clone()
	sent.Set("Anthropic-Beta", "oauth-2025-04-20,interleaved-thinking-2025-05-14")
	sent.Set("Anthropic-Version", "2023-06-01")
	resp, body := do(t, g.client, "POST", "https://"+up+"/v1/messages?beta=true", sent)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("%s: %s", resp.Status, body)
	}
	var seen http.Header
	_ = json.Unmarshal([]byte(body), &seen)
	if seen.Get(header) != want {
		t.Errorf("the model API got a %s of %d bytes, not the credential", header, len(seen.Get(header)))
	}
	if strings.Contains(body, placeholder) {
		t.Errorf("the placeholder reached the model API")
	}
	if seen.Get("Anthropic-Beta") != sent.Get("Anthropic-Beta") || seen.Get("Anthropic-Version") != "2023-06-01" {
		t.Errorf("the vendor headers changed on the way: %q, %q", seen.Get("Anthropic-Beta"), seen.Get("Anthropic-Version"))
	}
}

// TestAPlaceholderInAHeaderTheRuleDoesNotFillIsRefused is a desk set up for a bearer token whose rule fills a key
// header, and a rule whose credential is not set: neither request leaves the proxy.
func TestAPlaceholderInAHeaderTheRuleDoesNotFillIsRefused(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	key := withCred(rule("model", up, "*"), "model", wire.Inject{Header: "X-Api-Key", Form: "{secret}"})
	unset := key
	unset.Set = false
	for i, r := range []wire.PolicyRule{key, unset} {
		g.portal.set(policy(i+1, r))
		waitFor(t, "the policy", func() bool { return g.p.Refresh(context.Background()) == nil })
		resp, body := do(t, g.client, "POST", "https://"+up+"/v1/messages", http.Header{"Authorization": {"Bearer " + placeholder}})
		if resp.StatusCode != http.StatusBadGateway || !strings.Contains(body, "Authorization") {
			t.Fatalf("credential set %v: %s: %s", r.Set, resp.Status, body)
		}
	}
}
