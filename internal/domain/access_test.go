package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestTheBoxProxyGetsTheRulesAndTheDeskProxyTheModelRules(t *testing.T) {
	inject := domain.HeaderInject{Name: "x-api-key", Form: "{secret}"}
	s := domain.Settings{
		Credentials: []domain.Credential{{Name: "anthropic", Value: domain.SecretSet}, {Name: "staging", Value: domain.SecretNotSet}},
		Rules: []domain.Rule{
			{Name: "model", Hosts: []domain.HostPort{{Host: "api.anthropic.com", Port: 443}}, Credential: "anthropic", Inject: inject, Allow: domain.Access{Any: true}},
			{Name: "staging", Hosts: []domain.HostPort{{Host: "api.example.com", Port: 443}}, Credential: "staging", Inject: inject, Allow: domain.Access{Any: true}},
		},
	}
	portal := domain.AnyRequest("portal", "portal.example.com", 443)
	repo := domain.Repo{Host: "github.com", Path: "example-org/shop-web"}
	box := s.PolicyFor(domain.BoxProxy, 7, portal, repo)
	if box.Version != 7 || box.Repo != repo || len(box.Rules) != 1 || box.Rules[0].Name != "staging" || box.Gives("staging") || box.Gives("anthropic") {
		t.Fatalf("box policy %+v", box)
	}
	desk := s.PolicyFor(domain.DeskProxy, 7, portal, repo)
	if len(desk.Rules) != 2 || desk.Rules[0].Name != "portal" || desk.Rules[1].Name != "model" || !desk.Gives("anthropic") {
		t.Fatalf("desk policy %+v", desk)
	}
}

func TestABoxVariableIsAFakeTokenOrTextWithTheRepository(t *testing.T) {
	r := domain.Rule{Name: "github-api", Credential: "github"}
	repo := domain.Repo{Host: "github.com", Path: "example-org/shop-web"}
	if got := r.BoxValue(domain.EnvVar{Name: "GH_TOKEN", Value: "placeholder"}, repo); got != "moor-placeholder-github-api" {
		t.Errorf("placeholder: %q", got)
	}
	if got := r.BoxValue(domain.EnvVar{Name: "GH_REPO", Value: "{repo}"}, repo); got != "example-org/shop-web" {
		t.Errorf("repo: %q", got)
	}
}

func TestSuggestTemplatesTheRepositoryAndNeverAQuery(t *testing.T) {
	repo := domain.Repo{Host: "github.com", Path: "example-org/shop-web"}
	cases := map[string]string{
		"/repos/example-org/shop-web/issues/3": "DELETE /repos/{repo}/issues/3",
		"/repos/Example-Org/Shop-Web/pulls":    "DELETE /repos/{repo}/pulls",
		"/example-org/shop-web.git/info/refs":  "DELETE /{repo}.git/info/refs",
		"/x":                                   "DELETE /x",
		"/a/*":                                 "",
		"/a/{b}":                               "",
		"/a/../b":                              "",
	}
	for path, want := range cases {
		if got := domain.Suggest("DELETE", path, repo); got != want {
			t.Errorf("%s: got %q, want %q", path, got, want)
		}
	}
}

func TestABlockedReportMustBeOneAProxyCouldMake(t *testing.T) {
	good := domain.BlockedHit{
		Proxy: domain.BoxProxy, Host: "api.github.com", Port: 443, Method: "POST", Path: "/graphql", Rule: "github-api",
		Reason: domain.DenyMerge, Detail: "merging is not allowed from a task (mutation mergePullRequest)",
	}
	if err := good.Validate(); err != nil {
		t.Fatal(err)
	}
	tunnel := domain.BlockedHit{Proxy: domain.BoxProxy, Host: "later.example", Port: 443, Method: domain.TunnelMethod, Entry: domain.AnyText, Reason: domain.DenyHost}
	if err := tunnel.Validate(); err != nil {
		t.Fatal(err)
	}
	bad := []func(*domain.BlockedHit){
		func(h *domain.BlockedHit) { h.Host = "*.example.com" },
		func(h *domain.BlockedHit) { h.Reason = "because" },
		func(h *domain.BlockedHit) { h.Method = "get" },
		func(h *domain.BlockedHit) { h.Path = "x" },
		func(h *domain.BlockedHit) { h.Entry = "GET x" },
		func(h *domain.BlockedHit) { h.Rule = "Not A Name" },
		func(h *domain.BlockedHit) { h.Detail = "a\nb" },
	}
	for i, change := range bad {
		h := good
		change(&h)
		if err := h.Validate(); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("change %d: err = %v", i, err)
		}
	}
}

func TestOnlyABoxRequestTheRulesRefusedCanBeAllowed(t *testing.T) {
	row := domain.BlockedRequest{Proxy: domain.BoxProxy, Host: "later.example", Port: 80, Method: "GET", Path: "/x", Entry: "GET /x", Reason: domain.DenyHost}
	if !row.Allowable() || row.Request() != "later.example:80 GET /x" {
		t.Fatalf("%+v: allowable %v, request %q", row, row.Allowable(), row.Request())
	}
	merge, desk, none := row, row, row
	merge.Reason, desk.Proxy, none.Entry = domain.DenyMerge, domain.DeskProxy, ""
	for _, r := range []domain.BlockedRequest{merge, desk, none} {
		if r.Allowable() {
			t.Errorf("%+v is allowable", r)
		}
	}
}
