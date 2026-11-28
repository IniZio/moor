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

var accessDoc = `repositories:
  - example-org/shop-web
credentials:
  staging:
    value: ` + stagingValue + `
  anthropic:
    value: ` + modelValue + `
rules:
  staging:
    hosts: [api.example.com]
    credential: staging
    inject: {header: Authorization, form: "Bearer {secret}"}
    allow: ["GET /**"]
    ask: ["DELETE /items/*"]
  anthropic:
    hosts: [api.anthropic.com]
    credential: anthropic
    inject: {header: x-api-key, form: "{secret}"}
    allow: "*"
`

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
	box.has(t, http.StatusOK, `"GET /**"`, `"DELETE /items/*"`, `"credential":"staging","set":true`, `"path":"example-org/shop-web"`)
	box.lacks(t, stagingValue, modelValue, "anthropic", "portal.example.com")
	desk := a.do(asProxy(http.MethodGet, "/internal/policy/41", tk.mint("41", domain.DeskProxy), ""))
	desk.has(t, http.StatusOK, `"portal.example.com"`, `"api.anthropic.com"`, `"credential":"anthropic"`)
	desk.lacks(t, stagingValue, modelValue, "api.example.com")
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
		return a.do(asProxy(http.MethodPost, "/internal/credentials/value", tk.mint("41", kind), `{"task":"41","credential":"`+name+`"}`))
	}
	ask(domain.BoxProxy, "staging").has(t, http.StatusOK, stagingValue)
	ask(domain.DeskProxy, "anthropic").has(t, http.StatusOK, modelValue)
	for _, got := range []reply{ask(domain.BoxProxy, "anthropic"), ask(domain.DeskProxy, "staging")} {
		if got.code != http.StatusNotFound {
			t.Errorf("a proxy got another proxy's value: %d", got.code)
		}
		got.lacks(t, stagingValue, modelValue)
	}
}

type blockedSnap struct {
	Blocked []struct {
		ID      int64  `json:"id"`
		Count   int    `json:"count"`
		Request string `json:"request"`
	} `json:"blocked_requests"`
}

func (a *app) allowBlocked(n int) (int, current) {
	a.t.Helper()
	var snap blockedSnap
	got := a.do(get("Mia", "/api/tasks/41"))
	if err := json.Unmarshal([]byte(got.body), &snap); err != nil || len(snap.Blocked) <= n {
		a.t.Fatalf("snapshot %v: %s", err, got.body)
	}
	path := "/api/tasks/41/blocked/" + strconv.FormatInt(snap.Blocked[n].ID, 10)
	allowed := a.do(req{method: http.MethodPost, path: path, person: "Ana", json: true, body: `{"action":"allow"}`})
	var line struct{ Line, Version int }
	if err := json.Unmarshal([]byte(allowed.body), &line); err != nil || allowed.code != http.StatusOK {
		a.t.Fatalf("allow: %d %s", allowed.code, allowed.body)
	}
	cur := a.current()
	if cur.Version != line.Version {
		a.t.Fatalf("allow saved v%d, the newest is v%d", line.Version, cur.Version)
	}
	return line.Line, cur
}

func TestABlockedRequestShowsOnTheTaskAndAllowAddsARule(t *testing.T) {
	a, tk := accessApp(t)
	report := `{"task":"41","host":"fonts.example.com","port":443,"method":"GET","path":"/css","entry":"GET /css","reason":"host_not_allowed"}`
	for range 2 {
		a.do(asProxy(http.MethodPost, "/internal/blocked", tk.mint("41", domain.BoxProxy), report)).has(t, http.StatusNoContent)
	}
	a.do(get("Mia", "/tasks/41")).has(t, http.StatusOK, "1 blocked request", "fonts.example.com GET /css", "Allow")
	line, cur := a.allowBlocked(0)
	if lines := strings.Split(cur.Text, "\n"); line < 1 || lines[line-1] != "      - GET /css" {
		t.Fatalf("line %d of v%d:\n%s", line, cur.Version, cur.Text)
	}
	a.do(asProxy(http.MethodGet, "/internal/policy/41", tk.mint("41", domain.BoxProxy), "")).has(t, http.StatusOK, `"fonts.example.com"`, `"GET /css"`)
	a.do(get("Mia", "/tasks/41")).has(t, http.StatusOK, "Blocked requests, all allowed", "Allowed")
}

func TestAllowOnAnAskedRequestMovesItsEntryToAllow(t *testing.T) {
	a, tk := accessApp(t)
	report := `{"task":"41","host":"api.example.com","port":443,"method":"DELETE","path":"/items/3","rule":"staging","entry":"DELETE /items/*","reason":"needs_approval"}`
	a.do(asProxy(http.MethodPost, "/internal/blocked", tk.mint("41", domain.BoxProxy), report)).has(t, http.StatusNoContent)
	line, cur := a.allowBlocked(0)
	if lines := strings.Split(cur.Text, "\n"); !strings.Contains(lines[line-1], "DELETE /items/*") || strings.Contains(cur.Text, "ask:") {
		t.Fatalf("line %d of v%d:\n%s", line, cur.Version, cur.Text)
	}
}

func TestAMergeShowsAsNeverAllowed(t *testing.T) {
	a, tk := accessApp(t)
	report := `{"task":"41","host":"api.github.com","port":443,"method":"POST","path":"/graphql","rule":"github-api","reason":"merge_not_allowed","detail":"merging is not allowed from a task (mutation mergePullRequest)"}`
	a.do(asProxy(http.MethodPost, "/internal/blocked", tk.mint("41", domain.BoxProxy), report)).has(t, http.StatusNoContent)
	page := a.do(get("Mia", "/tasks/41"))
	page.has(t, http.StatusOK, "Merging is never allowed")
	a.do(get("Mia", "/api/tasks/41")).has(t, http.StatusOK, "mergePullRequest")
}
