package main

import (
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// Synthetic values, never real ones.
const (
	stagingValue = "synthetic-staging-7d1e"
	modelValue   = "synthetic-model-93ab"
)

var accessDoc = "repositories:\n  - example-org/shop-web\nallow:\n  - api.example.com:443 GET\nconnections:\n" +
	"  - name: Staging key\n    hosts: [api.example.com]\n    add: header Authorization \"Bearer {value}\"\n    value: " + stagingValue + "\n" +
	"  - name: Model key\n    hosts: [api.anthropic.com]\n    add: header x-api-key \"{value}\"\n    value: " + modelValue + "\n"

func accessApp(t *testing.T) (*app, tokens) {
	t.Helper()
	a := newApp(t, func(c *config) { c.publicURL = "https://portal.example.com" })
	a.do(put("Mia", 0, accessDoc)).has(t, http.StatusOK)
	a.seed(seeded("41", 1, domain.Running, "Mia"), seeded("42", 2, domain.Done, "Mia"))
	return a, newTokens(a.key)
}

func asProxy(method, path, token, body string) req {
	return req{method: method, path: path, json: body != "", body: body, headers: map[string]string{"Authorization": "Bearer " + token}}
}

func TestEachProxyReadsItsOwnPolicyWithoutValues(t *testing.T) {
	a, tk := accessApp(t)
	box := a.do(asProxy(http.MethodGet, "/internal/policy/41", tk.mint("41", domain.BoxProxy), ""))
	box.has(t, http.StatusOK, `"api.example.com:443 GET"`, "Staging key")
	box.lacks(t, stagingValue, modelValue, "Model key", "portal.example.com")
	desk := a.do(asProxy(http.MethodGet, "/internal/policy/41", tk.mint("41", domain.DeskProxy), ""))
	desk.has(t, http.StatusOK, "portal.example.com:443 GET", "api.anthropic.com:443 GET", "Model key")
	desk.lacks(t, stagingValue, modelValue, "Staging key")
	for name, r := range map[string]req{
		"no token":         asProxy(http.MethodGet, "/internal/policy/41", "", ""),
		"another task's":   asProxy(http.MethodGet, "/internal/policy/41", tk.mint("42", domain.BoxProxy), ""),
		"an ended task's":  asProxy(http.MethodGet, "/internal/policy/42", tk.mint("42", domain.BoxProxy), ""),
		"a forged kind":    asProxy(http.MethodGet, "/internal/policy/41", "desk."+strings.SplitN(tk.mint("41", domain.BoxProxy), ".", 2)[1], ""),
		"a person, signed": {method: http.MethodGet, path: "/internal/policy/41", person: "Mia"},
	} {
		if got := a.do(r); got.code != http.StatusUnauthorized {
			t.Errorf("%s: %d %s", name, got.code, got.body)
		}
	}
}

func TestAValueLeavesThePortalOnlyForTheProxyThatAddsIt(t *testing.T) {
	a, tk := accessApp(t)
	ask := func(kind domain.ProxyKind, name string) reply {
		return a.do(asProxy(http.MethodPost, "/internal/connections/value", tk.mint("41", kind), `{"task":"41","connection":"`+name+`"}`))
	}
	ask(domain.BoxProxy, "Staging key").has(t, http.StatusOK, stagingValue)
	ask(domain.DeskProxy, "Model key").has(t, http.StatusOK, modelValue)
	for _, got := range []reply{ask(domain.BoxProxy, "Model key"), ask(domain.DeskProxy, "Staging key")} {
		if got.code != http.StatusNotFound {
			t.Errorf("a proxy got another proxy's value: %d", got.code)
		}
		got.lacks(t, stagingValue, modelValue)
	}
}

func TestABlockedRequestShowsOnTheTaskAndAllowAddsALine(t *testing.T) {
	a, tk := accessApp(t)
	report := `{"task":"41","host":"fonts.example.com","port":443,"method":"GET","reason":"host_not_allowed"}`
	for range 2 {
		a.do(asProxy(http.MethodPost, "/internal/blocked", tk.mint("41", domain.BoxProxy), report)).has(t, http.StatusNoContent)
	}
	a.do(get("Mia", "/tasks/41")).has(t, http.StatusOK, "1 blocked request", "fonts.example.com:443 GET", "Allow")
	var snap struct {
		Blocked []struct {
			ID    int64 `json:"id"`
			Count int   `json:"count"`
		} `json:"blocked_requests"`
	}
	got := a.do(get("Mia", "/api/tasks/41"))
	if err := json.Unmarshal([]byte(got.body), &snap); err != nil || len(snap.Blocked) != 1 || snap.Blocked[0].Count != 2 {
		t.Fatalf("snapshot %v: %s", err, got.body)
	}
	path := "/api/tasks/41/blocked/" + strconv.FormatInt(snap.Blocked[0].ID, 10)
	allowed := a.do(req{method: http.MethodPost, path: path, person: "Ana", json: true, body: `{"action":"allow"}`})
	var line struct{ Line, Version int }
	if err := json.Unmarshal([]byte(allowed.body), &line); err != nil || allowed.code != http.StatusOK {
		t.Fatalf("allow: %d %s", allowed.code, allowed.body)
	}
	cur := a.current()
	if lines := strings.Split(cur.Text, "\n"); line.Line < 1 || lines[line.Line-1] != "  - fonts.example.com:443 GET" || cur.Version != line.Version {
		t.Fatalf("line %d of v%d:\n%s", line.Line, cur.Version, cur.Text)
	}
	a.do(asProxy(http.MethodGet, "/internal/policy/41", tk.mint("41", domain.BoxProxy), "")).has(t, http.StatusOK, "fonts.example.com:443 GET")
	a.do(get("Mia", "/tasks/41")).has(t, http.StatusOK, "Blocked requests, all allowed", "Allowed")
}
