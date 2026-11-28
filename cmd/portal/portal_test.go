package main

import (
	"context"
	"encoding/base64"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/secretbox"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
)

const (
	userHeader = "X-Test-User"
	testKeyB64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
)

type app struct {
	t       *testing.T
	handler http.Handler
	dsn     string
	key     []byte
}

func newApp(t *testing.T) *app {
	t.Helper()
	dsn := pgtest.SchemaDSN(t)
	key, err := base64.StdEncoding.DecodeString(testKeyB64)
	if err != nil || len(key) != secretbox.KeySize {
		t.Fatalf("test key: %v", err)
	}
	cfg := config{listen: "127.0.0.1:0", database: dsn, userHeader: userHeader, orgName: "example-org", secretKey: key}
	handler, closeApp, err := build(context.Background(), cfg)
	if err != nil {
		t.Fatalf("build: %v", err)
	}
	t.Cleanup(closeApp)
	return &app{t: t, handler: handler, dsn: dsn, key: key}
}

// req is one request. Header names the person unless Anon is set.
type req struct {
	method, path string
	person       string
	body         string
	json         bool
	form         url.Values
	headers      map[string]string
}

type reply struct {
	code   int
	body   string
	header http.Header
}

func (a *app) do(r req) reply {
	a.t.Helper()
	var body io.Reader
	switch {
	case r.json:
		body = strings.NewReader(r.body)
	case r.form != nil:
		body = strings.NewReader(r.form.Encode())
	}
	hr := httptest.NewRequest(r.method, r.path, body)
	if r.person != "" {
		hr.Header.Set(userHeader, r.person)
	}
	switch {
	case r.json:
		hr.Header.Set("Content-Type", "application/json")
	case r.form != nil:
		hr.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	for k, v := range r.headers {
		hr.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	a.handler.ServeHTTP(rec, hr)
	return reply{code: rec.Code, body: rec.Body.String(), header: rec.Header()}
}

func (r reply) has(t *testing.T, code int, parts ...string) {
	t.Helper()
	if r.code != code {
		t.Fatalf("status %d, want %d, body %q", r.code, code, r.body)
	}
	for _, p := range parts {
		if !strings.Contains(r.body, p) {
			t.Errorf("body lacks %q: %s", p, r.body)
		}
	}
}

func (r reply) lacks(t *testing.T, parts ...string) {
	t.Helper()
	for _, p := range parts {
		if strings.Contains(r.body, p) {
			t.Errorf("body holds %q: %s", p, r.body)
		}
	}
}

func get(person, path string) req { return req{method: http.MethodGet, path: path, person: person} }

func TestEveryRouteButTheHealthChecksNeedsAPerson(t *testing.T) {
	a := newApp(t)
	for _, r := range []req{
		get("", "/"), get("", "/tasks"), get("", "/machines"), get("", "/settings"), get("", "/api/me"),
		get("", "/api/settings"), get("", "/api/settings/versions"), get("", "/api/settings/versions/1"),
		{method: http.MethodPut, path: "/api/settings", json: true, body: "{}"},
		{method: http.MethodPost, path: "/api/settings/check", json: true, body: "{}"},
		get("   ", "/api/me"),
	} {
		a.do(r).has(t, http.StatusUnauthorized)
	}
	for _, path := range []string{"/healthz", "/readyz", "/static/style.css", "/static/htmx.min.js", "/static/editor.js"} {
		a.do(get("", path)).has(t, http.StatusOK)
	}
}

func TestMeNamesThePerson(t *testing.T) {
	a := newApp(t)
	a.do(get("Mia", "/api/me")).has(t, http.StatusOK, `"name":"Mia"`)
}

func TestTabsPlaceholdersAndRedirect(t *testing.T) {
	a := newApp(t)
	if r := a.do(get("Mia", "/")); r.code != http.StatusFound || r.header.Get("Location") != "/tasks" {
		t.Errorf("/ gave %d to %q", r.code, r.header.Get("Location"))
	}
	for path, current := range map[string]string{"/tasks": "Tasks", "/machines": "Machines"} {
		r := a.do(get("Mia", path))
		r.has(t, http.StatusOK, "Nothing here yet.", "Tasks</a>", "Settings</a>", "Machines</a>", `<span class="me" title="Mia">M</span>`)
		if !strings.Contains(r.body, "aria-current href=\"/"+strings.ToLower(current)+"\"") && !strings.Contains(r.body, "href=\"/"+strings.ToLower(current)+"\" aria-current") {
			t.Errorf("%s does not mark %s as current: %s", path, current, r.body)
		}
	}
}

func TestPagesLoadNothingFromOutside(t *testing.T) {
	a := newApp(t)
	for _, path := range []string{"/tasks", "/settings"} {
		r := a.do(get("Mia", path))
		r.has(t, http.StatusOK)
		r.lacks(t, "src=\"http", "href=\"http", "https://")
	}
}
