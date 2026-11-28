package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

type seen struct {
	mu    sync.Mutex
	auth  []string
	paths []string
	keys  []string
	lines []string
	pages []string
}

func (s *seen) count() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.keys)
}

// portal answers each call with the next status, and the last one for every call after.
func portal(t *testing.T, statuses ...int) (*httptest.Server, *seen) {
	t.Helper()
	s := &seen{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var in struct{ Line, Page, Key string }
		_ = json.NewDecoder(r.Body).Decode(&in)
		s.mu.Lock()
		s.auth, s.paths = append(s.auth, r.Header.Get("Authorization")), append(s.paths, r.Method+" "+r.URL.Path)
		s.keys, s.lines, s.pages = append(s.keys, in.Key), append(s.lines, in.Line), append(s.pages, in.Page)
		status := statuses[min(len(s.keys), len(statuses))-1]
		s.mu.Unlock()
		if status != http.StatusOK {
			http.Error(w, "refused "+http.StatusText(status), status)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"version": 2, "url": "https://portal.example/results/7/2", "state": "running"})
	}))
	t.Cleanup(srv.Close)
	return srv, s
}

func env(srv *httptest.Server) func(string) string {
	vars := map[string]string{"MOOR_PORTAL": srv.URL, "MOOR_TASK": "7", "MOOR_TASK_TOKEN": "task.secret"}
	return func(k string) string { return vars[k] }
}

func pageFile(t *testing.T, html string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), "page.html")
	if err := os.WriteFile(p, []byte(html), 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestPublishPostsThePageAndItsVerdictAsTheTask(t *testing.T) {
	srv, s := portal(t, http.StatusOK)
	var out, errw bytes.Buffer
	file := pageFile(t, "<h1>Checkout</h1><p>5 < 6 & ok</p>")
	if code := run([]string{file, "--verdict", "Five of five flows passed."}, env(srv), &out, &errw); code != 0 {
		t.Fatalf("exit %d: %s", code, errw.String())
	}
	if s.paths[0] != "POST /api/tasks/7/results" || s.auth[0] != "Bearer task.secret" || s.lines[0] != "Five of five flows passed." {
		t.Errorf("the call was %v %v %v", s.paths, s.auth, s.lines)
	}
	if s.pages[0] != "<h1>Checkout</h1><p>5 < 6 & ok</p>" || s.keys[0] == "" {
		t.Errorf("page %q key %q", s.pages[0], s.keys[0])
	}
	if !strings.Contains(out.String(), "published v2") || !strings.Contains(out.String(), "https://portal.example/results/7/2") {
		t.Errorf("output %q", out.String())
	}
}

func TestTheVerdictMayComeFirstOrLast(t *testing.T) {
	srv, s := portal(t, http.StatusOK)
	file := pageFile(t, "<p>x</p>")
	for _, args := range [][]string{{"--verdict", "ok", file}, {"--verdict=ok", file}, {file, "-verdict", "ok"}} {
		var out, errw bytes.Buffer
		if code := run(args, env(srv), &out, &errw); code != 0 {
			t.Fatalf("%v: exit %d: %s", args, code, errw.String())
		}
	}
	if s.count() != 3 {
		t.Fatalf("%d calls", s.count())
	}
}

func TestARetryAfterAServerErrorKeepsItsKeyAndANewRunGetsANewOne(t *testing.T) {
	srv, s := portal(t, http.StatusBadGateway, http.StatusInternalServerError, http.StatusOK)
	file := pageFile(t, "<p>x</p>")
	var out, errw bytes.Buffer
	if code := run([]string{file, "--verdict", "ok"}, env(srv), &out, &errw); code != 0 {
		t.Fatalf("exit %d: %s", code, errw.String())
	}
	if s.count() != 3 || s.keys[0] != s.keys[1] || s.keys[1] != s.keys[2] {
		t.Fatalf("%d calls with keys %v, want three with one key", s.count(), s.keys)
	}
	if code := run([]string{file, "--verdict", "ok"}, env(srv), &out, &errw); code != 0 {
		t.Fatal(errw.String())
	}
	if s.keys[3] == s.keys[0] {
		t.Error("a second run of the tool reused the first run's key, so it would make no new version")
	}
}

func TestARefusalIsNotRetriedAndSaysWhy(t *testing.T) {
	srv, s := portal(t, http.StatusConflict)
	var out, errw bytes.Buffer
	if code := run([]string{pageFile(t, "<p>x</p>"), "--verdict", "ok"}, env(srv), &out, &errw); code != 1 {
		t.Fatalf("exit %d", code)
	}
	if s.count() != 1 || !strings.Contains(errw.String(), "409") || !strings.Contains(errw.String(), "refused Conflict") {
		t.Fatalf("%d calls, stderr %q", s.count(), errw.String())
	}
}

func TestWhatIsMissingIsSaidAndNothingIsSent(t *testing.T) {
	srv, s := portal(t, http.StatusOK)
	file := pageFile(t, "<p>x</p>")
	for name, c := range map[string]struct {
		args []string
		env  func(string) string
		want string
	}{
		"no file":      {[]string{"--verdict", "ok"}, env(srv), "usage"},
		"no verdict":   {[]string{file}, env(srv), "--verdict"},
		"two files":    {[]string{file, file, "--verdict", "ok"}, env(srv), "usage"},
		"missing file": {[]string{file + ".nope", "--verdict", "ok"}, env(srv), "page.html.nope"},
		"empty file":   {[]string{pageFile(t, ""), "--verdict", "ok"}, env(srv), "empty"},
		"no token":     {[]string{file, "--verdict", "ok"}, func(k string) string { return map[string]string{"MOOR_PORTAL": srv.URL, "MOOR_TASK": "7"}[k] }, "MOOR_TASK_TOKEN"},
	} {
		var out, errw bytes.Buffer
		if code := run(c.args, c.env, &out, &errw); code != 2 && code != 1 || !strings.Contains(errw.String(), c.want) {
			t.Errorf("%s: exit %d, stderr %q, want it to say %q", name, code, errw.String(), c.want)
		}
	}
	if s.count() != 0 {
		t.Errorf("%d calls were sent", s.count())
	}
}

func TestAPageThatTheCapRefusesIsNotSent(t *testing.T) {
	srv, s := portal(t, http.StatusOK)
	var out, errw bytes.Buffer
	big := pageFile(t, strings.Repeat("a", maxPage+1))
	if code := run([]string{big, "--verdict", "ok"}, env(srv), &out, &errw); code == 0 || !strings.Contains(errw.String(), "over") || s.count() != 0 {
		t.Fatalf("exit %d, stderr %q, %d calls", code, errw.String(), s.count())
	}
}
