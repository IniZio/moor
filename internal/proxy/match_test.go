package proxy

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func entry(t *testing.T, s string) domain.Entry {
	t.Helper()
	e, err := domain.ParseEntry(s)
	if err != nil {
		t.Fatal(err)
	}
	return e
}

func TestDecideRefusesMergesAndFoldedPathsWhateverTheRules(t *testing.T) {
	gh := domain.Rule{Name: "gh", Hosts: []domain.HostPort{{Host: "api.github.com", Port: 443}}, Allow: domain.Access{Entries: []domain.Entry{
		entry(t, "GET /repos/{repo}/**"), entry(t, "GET|POST|PUT /repos/{repo}/pulls/**"),
	}}, Ask: []domain.Entry{entry(t, "DELETE /repos/{repo}/**")}}
	anything := domain.Rule{Name: "any", Hosts: []domain.HostPort{{Host: "*.example.com", Port: 443}}, Allow: domain.Access{Any: true}}
	pol := &domain.Policy{Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Rules: []domain.Rule{gh, anything}}
	cases := []struct {
		host   string
		method domain.Method
		path   string
		reason domain.DenyReason
		entry  string
	}{
		{"api.github.com", "GET", "/repos/example-org/shop-web", "", ""},
		{"api.github.com", "GET", "/repos/Example-Org/Shop-Web/issues/3", "", ""},
		{"api.github.com", "POST", "/repos/example-org/shop-web/pulls", "", ""},
		{"api.github.com", "GET", "/repos/example-org/other/issues", domain.DenyRequest, "GET /repos/example-org/other/issues"},
		{"api.github.com", "PUT", "/repos/example-org/shop-web/pulls/9/merge", domain.DenyMerge, ""},
		{"api.github.com", "PUT", "/repos/example-org/shop-web/pulls/9/merge/", domain.DenyMerge, ""},
		{"a.example.com", "PUT", "/api/v3/repos/o/r/pulls/9/merge", domain.DenyMerge, ""},
		{"a.example.com", "POST", "/repos/o/r/merges", domain.DenyMerge, ""},
		{"api.github.com", "DELETE", "/repos/example-org/shop-web/labels/bug", domain.DenyAsk, "DELETE /repos/{repo}/**"},
		{"api.github.com", "GET", "/repos/example-org/shop-web/../other/x", domain.DenyRequest, ""},
		{"api.github.com", "GET", "/repos/example-org/shop-web/pulls/9%2fmerge", domain.DenyRequest, ""},
		{"api.github.com", "GET", "/repos/example-org/shop-web//x", domain.DenyRequest, ""},
		{"b.example.com", "DELETE", "/anything", "", ""},
		{"other.test", "GET", "/x", domain.DenyHost, "GET /x"},
	}
	for _, c := range cases {
		v := decide(pol, c.host, 443, c.method, c.path)
		if c.reason == "" {
			if v.pass == nil {
				t.Errorf("%s %s%s refused: %s", c.method, c.host, c.path, v.reason)
			}
			continue
		}
		if v.pass != nil || v.reason != c.reason || v.entry != c.entry {
			t.Errorf("%s %s%s: got %+v, want %s with entry %q", c.method, c.host, c.path, v, c.reason, c.entry)
		}
	}
}

func TestOnlyAnUntouchedRuleTunnels(t *testing.T) {
	anyRule := domain.Rule{Allow: domain.Access{Any: true}}
	withCred := domain.Rule{Allow: domain.Access{Any: true}, Credential: "c"}
	paths := domain.Rule{Allow: domain.Access{Entries: []domain.Entry{entry(t, "GET /**")}}}
	if intercepts([]domain.Rule{anyRule}, "db.example.com") {
		t.Error("an any rule without a credential is intercepted")
	}
	for name, rules := range map[string][]domain.Rule{"credential": {withCred}, "paths": {paths}, "mixed": {anyRule, paths}} {
		if !intercepts(rules, "db.example.com") {
			t.Errorf("a %s rule tunnels", name)
		}
	}
	if !intercepts([]domain.Rule{anyRule}, githubAPI) {
		t.Error("GitHub's API tunnels, so a merge would pass unseen")
	}
}
