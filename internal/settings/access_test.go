package settings_test

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/settings"
)

var (
	portal = domain.AnyRequest("portal", "portal.example.com", 443)
	repo   = domain.Repo{Host: "github.com", Path: "example-org/shop-web"}
)

func TestAProxyReadsThePolicyOfTheNewestVersion(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "rule api api.example.com -\n")
	p, err := f.svc.Policy(ctx, domain.BoxProxy, portal, repo)
	if err != nil || p.Version != 1 || len(p.Rules) != 1 || p.Repo != repo {
		t.Fatalf("policy %+v %v", p, err)
	}
	f.save(t, 1, "rule api api.example.com -\nrule search search.example.com -\n")
	p, err = f.svc.Policy(ctx, domain.BoxProxy, portal, repo)
	if err != nil || len(p.Rules) != 2 || p.Version != 2 {
		t.Fatalf("after the save the policy is %+v, %v", p, err)
	}
}

func TestAValueGoesOnlyToTheProxyThatAddsIt(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "secret stagingkey s3cr3t\nsecret modelkey m0d3l\n")
	f.save(t, 1, "cred stagingkey set\ncred modelkey set\ncred unsetkey not_set\ncred idle set\n"+
		"rule staging api.example.com stagingkey\nrule model api.anthropic.com modelkey\nrule unset other.example.com unsetkey\n")
	if v, err := f.svc.Value(ctx, domain.BoxProxy, repo, "stagingkey"); err != nil || v.Value != "s3cr3t" {
		t.Fatalf("the Box proxy got %v for its credential", err)
	}
	if v, err := f.svc.Value(ctx, domain.DeskProxy, repo, "modelkey"); err != nil || v.Value != "m0d3l" {
		t.Fatalf("the desk proxy got %v for the model credential", err)
	}
	for _, c := range []struct {
		kind domain.ProxyKind
		name string
	}{{domain.BoxProxy, "modelkey"}, {domain.DeskProxy, "stagingkey"}, {domain.BoxProxy, "unsetkey"}, {domain.BoxProxy, "idle"}, {domain.BoxProxy, "nokey"}} {
		if v, err := f.svc.Value(ctx, c.kind, repo, c.name); !errors.Is(err, domain.ErrNotFound) || v.Value != "" {
			t.Errorf("the %s proxy asking for %s got a value or %v", c.kind, c.name, err)
		}
	}
}

func TestAGitHubAppCredentialMintsATokenForTheTasksRepository(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	if _, err := f.svc.GitHubApp(ctx); !errors.Is(err, settings.ErrNoApp) {
		t.Fatalf("no App yet: %v", err)
	}
	f.save(t, 0, "app github set\nsecretkey github/private_key PEM\nrule gh api.github.com github\n")
	v, err := f.svc.Value(ctx, domain.BoxProxy, repo, "github")
	if err != nil || v.Value != "minted-example-org/shop-web" || v.Expires.IsZero() {
		t.Fatalf("value %+v %v", v, err)
	}
	if want := []string{"app 7 key pem for example-org/shop-web"}; !reflect.DeepEqual(f.minter.asked, want) {
		t.Fatalf("minter asked %v", f.minter.asked)
	}
	keys, err := f.svc.GitHubApp(ctx)
	if err != nil || keys.AppID != 7 || string(keys.PrivateKey) != "pem" || keys.WebhookSecret != nil || keys.API() != domain.GitHubAPI {
		t.Fatalf("app keys %+v %v", keys, err)
	}
}

func hit(host string, method domain.Method) domain.BlockedHit {
	return domain.BlockedHit{Task: "7", Proxy: domain.BoxProxy, Host: host, Port: 443, Method: method, Path: "/css", Entry: "GET /css", Reason: domain.DenyHost}
}

func TestABlockedRequestIsCountedAndAllowAddsItsEntry(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "rule api api.example.com -\n")
	for range 3 {
		if err := f.svc.RecordBlocked(ctx, hit("fonts.example.com", "GET")); err != nil {
			t.Fatal(err)
		}
	}
	if err := f.svc.RecordBlocked(ctx, hit("*.example.com", "GET")); err == nil {
		t.Fatal("a bad report was stored")
	}
	list, err := f.svc.BlockedRequests(ctx, "7")
	if err != nil || len(list) != 1 || list[0].Count != 3 || list[0].Request() != "fonts.example.com GET /css" {
		t.Fatalf("blocked %+v %v", list, err)
	}
	allowed(t, f, list[0].ID)
}

func allowed(t *testing.T, f fixture, id domain.BlockedID) {
	t.Helper()
	ctx := context.Background()
	if _, err := f.svc.Allow(ctx, "8", id, "Mia"); !errors.Is(err, domain.ErrNotFound) {
		t.Fatalf("allow from another task: %v", err)
	}
	got, err := f.svc.Allow(ctx, "7", id, "Mia")
	if err != nil || got.Line != 2 || got.Version != 2 {
		t.Fatalf("allow: %+v %v", got, err)
	}
	cur, _ := f.svc.Current(ctx)
	if cur.Text != "rule api api.example.com -\nentry fonts.example.com GET /css\n" || cur.SavedBy != "Mia" || cur.Note != "Allow on blocked request fonts.example.com GET /css of task #7" {
		t.Fatalf("saved %+v", cur)
	}
	list, _ := f.svc.BlockedRequests(ctx, "7")
	if list[0].Decision != domain.Allowed || list[0].DecidedBy != "Mia" {
		t.Fatalf("decision %+v", list[0])
	}
}

func TestAllowOfAnAskEntryNamesItsRule(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "rule gh api.github.com -\n")
	h := domain.BlockedHit{
		Task: "7", Proxy: domain.BoxProxy, Host: "api.github.com", Port: 443, Method: "DELETE", Path: "/repos/example-org/shop-web/labels/x",
		Rule: "gh", Entry: "DELETE /repos/{repo}/**", Reason: domain.DenyAsk,
	}
	if err := f.svc.RecordBlocked(ctx, h); err != nil {
		t.Fatal(err)
	}
	if _, err := f.svc.Allow(ctx, "7", 1, "Mia"); err != nil {
		t.Fatal(err)
	}
	if cur, _ := f.svc.Current(ctx); cur.Text != "rule gh api.github.com -\nentry gh DELETE /repos/{repo}/**\n" {
		t.Fatalf("saved %q", cur.Text)
	}
}

func TestADeskRequestOrAMergeCannotBeAllowedButCanBeDismissed(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	desk := hit("api.example.com", "GET")
	desk.Proxy = domain.DeskProxy
	merge := domain.BlockedHit{Task: "7", Proxy: domain.BoxProxy, Host: "api.github.com", Port: 443, Method: "PUT", Path: "/repos/a/b/pulls/1/merge", Rule: "gh", Reason: domain.DenyMerge}
	for _, h := range []domain.BlockedHit{desk, merge} {
		if err := f.svc.RecordBlocked(ctx, h); err != nil {
			t.Fatal(err)
		}
	}
	for _, id := range []domain.BlockedID{1, 2} {
		if _, err := f.svc.Allow(ctx, "7", id, "Mia"); !errors.Is(err, domain.ErrRefused) {
			t.Fatalf("allow of row %d: %v", id, err)
		}
	}
	if err := f.svc.Dismiss(ctx, "7", 1, "Mia"); err != nil {
		t.Fatal(err)
	}
	list, _ := f.svc.BlockedRequests(ctx, "7")
	if list[0].Decision != domain.Dismissed {
		t.Fatalf("decision %+v", list[0])
	}
	if cur, _ := f.svc.Current(ctx); cur.N != 0 {
		t.Fatalf("a dismiss saved settings v%d", cur.N)
	}
}

func TestTheInitialTextIsTheExampleWithNothingSet(t *testing.T) {
	for _, want := range []string{"credentials:", "rules:", "graphql_mutations:", "not set"} {
		if !strings.Contains(settings.InitialText, want) {
			t.Errorf("the initial text has no %q", want)
		}
	}
	if strings.Contains(settings.InitialText, ": set") {
		t.Errorf("the initial text says a secret is set")
	}
}
