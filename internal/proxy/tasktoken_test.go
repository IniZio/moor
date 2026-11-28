package proxy_test

import (
	"net/http"
	"testing"

	"github.com/IniZio/moor/internal/proxy"
)

const taskToken = "task.synthetic-5d1e"

func TestTheDeskProxyAddsTheTaskTokenOnlyToItsTasksPortalRoutes(t *testing.T) {
	g := newRig(t, policy(1), func(c *proxy.Config) { c.TaskToken = taskToken })
	g.portal.set(policy(2, rule("portal", hostPort(g.home), "*")))
	waitFor(t, "the portal rule", func() bool {
		resp, _ := do(t, g.client, http.MethodPost, g.home.URL+"/api/tasks/8/results", nil)
		return resp.StatusCode == http.StatusCreated
	})
	placeholder := http.Header{"Authorization": {"Bearer moor-placeholder-task"}}
	for _, path := range []string{"/api/tasks/7/results", "/api/tasks/8/results", "/api/tasks/7/input"} {
		if resp, _ := do(t, g.client, http.MethodPost, g.home.URL+path, placeholder); resp.StatusCode != http.StatusCreated {
			t.Fatalf("%s: %s", path, resp.Status)
		}
	}
	g.portal.mu.Lock()
	defer g.portal.mu.Unlock()
	want := map[string]string{
		"/api/tasks/7/results": "Bearer " + taskToken,
		"/api/tasks/8/results": "Bearer moor-placeholder-task",
		"/api/tasks/7/input":   "Bearer moor-placeholder-task",
	}
	for path, auth := range want {
		if got := g.portal.calls[path]; got != auth {
			t.Errorf("%s reached the portal with %q, want %q", path, got, auth)
		}
	}
}

func TestAProxyWithoutATaskTokenLeavesThePortalCallAsItIs(t *testing.T) {
	g := newRig(t, policy(1))
	g.portal.set(policy(2, rule("portal", hostPort(g.home), "*")))
	placeholder := http.Header{"Authorization": {"Bearer moor-placeholder-task"}}
	waitFor(t, "the portal rule", func() bool {
		resp, _ := do(t, g.client, http.MethodPost, g.home.URL+"/api/tasks/7/results", placeholder)
		return resp.StatusCode == http.StatusCreated
	})
	g.portal.mu.Lock()
	defer g.portal.mu.Unlock()
	if got := g.portal.calls["/api/tasks/7/results"]; got != "Bearer moor-placeholder-task" {
		t.Errorf("the call reached the portal with %q", got)
	}
}
