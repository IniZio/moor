package main

import (
	"encoding/json"
	"net/http"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/githome/githometest"
	"github.com/IniZio/moor/internal/adapter/github/githubtest"
)

func pushBody(ref, repo string) string {
	b, _ := json.Marshal(map[string]any{"ref": ref, "repository": map[string]any{"full_name": repo}})
	return string(b)
}

// TestAPushWebhookRefreshesThePortalsHomeWithAReadTokenForThatRepository drives a whole portal: its App is in
// the settings, its home is a repository on a git server, and GitHub's push webhook is what tells it to fetch.
func TestAPushWebhookRefreshesThePortalsHomeWithAReadTokenForThatRepository(t *testing.T) {
	fake := githubtest.NewForTest(t)
	key, err := githubtest.NewKey()
	if err != nil {
		t.Fatal(err)
	}
	git := githometest.New(t, homePath, "fake-token-4242")
	if _, err := git.Commit("main", map[string]string{"tasks/a.yaml": spec("alpha")}); err != nil {
		t.Fatal(err)
	}
	a := newApp(t, func(c *config) { c.home = homeConfig{repo: homeRepoName, every: time.Hour, gitBase: git.URL()} })
	specs := func() string { return a.do(get("Mia", "/api/task-specs")).body }

	eventually(t, "the reason there is no App", func() bool { return strings.Contains(specs(), "the settings hold no GitHub App") })
	a.do(get("Mia", "/tasks?spec=team+home")).has(t, http.StatusOK, "the settings hold no GitHub App")
	a.do(put("Mia", 0, appDoc(fake.URL(), key))).has(t, http.StatusOK)

	a.do(signed("push", "d-1", pushBody("refs/heads/main", "example-org/team-home"), hookSecret)).has(t, http.StatusAccepted)
	eventually(t, "the first spec", func() bool { return strings.Contains(specs(), `"name":"alpha"`) })

	if _, err := git.Commit("main", map[string]string{"tasks/a.yaml": spec("alpha"), "tasks/b.yaml": spec("beta")}); err != nil {
		t.Fatal(err)
	}
	a.do(signed("push", "d-2", pushBody("refs/heads/main", "example-org/team-home"), hookSecret)).has(t, http.StatusAccepted)
	eventually(t, "the pushed spec", func() bool { return strings.Contains(specs(), `"name":"beta"`) })

	for _, tr := range fake.TokenRequests() {
		want := githubtest.TokenRequest{Installation: "4242", Repositories: []string{"team-home"}, Permissions: map[string]string{"contents": "read"}}
		if !reflect.DeepEqual(tr, want) {
			t.Errorf("a token was asked for as %+v, want %+v", tr, want)
		}
	}
	if len(fake.TokenRequests()) == 0 {
		t.Fatal("no token was minted")
	}
}
