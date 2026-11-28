package settings_test

import (
	"context"
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

var portal = domain.AnyMethod("portal.example.com", 443)

func TestAProxyReadsThePolicyOfTheNewestVersion(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "allow api.example.com:443 GET\n")
	p, err := f.svc.Policy(ctx, domain.BoxProxy, portal)
	if err != nil || p.Version != 1 {
		t.Fatalf("policy %+v %v", p, err)
	}
	if _, ok := p.Check("search.example.com", 443, "GET"); ok {
		t.Fatal("an unlisted host passed")
	}
	f.save(t, 1, "allow api.example.com:443 GET\nallow search.example.com:443 GET\n")
	p, err = f.svc.Policy(ctx, domain.BoxProxy, portal)
	if _, ok := p.Check("search.example.com", 443, "GET"); err != nil || !ok || p.Version != 2 {
		t.Fatalf("after the save the policy is %+v, %v", p, err)
	}
}

func TestAValueGoesOnlyToTheProxyThatAddsIt(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "secret stagingkey s3cr3t\nsecret modelkey m0d3l\n")
	f.save(t, 1, "conn stagingkey api.example.com set\nconn modelkey api.anthropic.com set\nconn unsetkey other.example.com not_set\n")
	if v, err := f.svc.Value(ctx, domain.BoxProxy, "stagingkey"); err != nil || v != "s3cr3t" {
		t.Fatalf("the Box proxy got %v for its connection", err)
	}
	if v, err := f.svc.Value(ctx, domain.DeskProxy, "modelkey"); err != nil || v != "m0d3l" {
		t.Fatalf("the desk proxy got %v for the model connection", err)
	}
	for _, c := range []struct {
		kind domain.ProxyKind
		name string
	}{{domain.BoxProxy, "modelkey"}, {domain.DeskProxy, "stagingkey"}, {domain.BoxProxy, "unsetkey"}, {domain.BoxProxy, "nokey"}} {
		if v, err := f.svc.Value(ctx, c.kind, c.name); !errors.Is(err, domain.ErrNotFound) || v != "" {
			t.Errorf("the %s proxy asking for %s got a value or %v", c.kind, c.name, err)
		}
	}
}

func hit(host string, method domain.Method) domain.BlockedHit {
	return domain.BlockedHit{Task: "7", Proxy: domain.BoxProxy, Host: host, Port: 443, Method: method, Reason: domain.DenyHost}
}

func TestABlockedRequestIsCountedAndAllowTurnsItIntoALine(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	f.save(t, 0, "allow api.example.com:443 GET\n")
	for range 3 {
		if err := f.svc.RecordBlocked(ctx, hit("fonts.example.com", "GET")); err != nil {
			t.Fatal(err)
		}
	}
	if err := f.svc.RecordBlocked(ctx, hit("*.example.com", "GET")); err == nil {
		t.Fatal("a bad report was stored")
	}
	list, err := f.svc.BlockedRequests(ctx, "7")
	if err != nil || len(list) != 1 || list[0].Count != 3 || list[0].Request() != "fonts.example.com:443 GET" {
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
	if cur.Text != "allow api.example.com:443 GET\nallow fonts.example.com:443 GET\n" || cur.SavedBy != "Mia" || cur.Note != "Allow on blocked request fonts.example.com:443 GET of task #7" {
		t.Fatalf("saved %+v", cur)
	}
	p, _ := f.svc.Policy(ctx, domain.BoxProxy, portal)
	if _, ok := p.Check("fonts.example.com", 443, "GET"); !ok {
		t.Fatal("the allowed request does not pass")
	}
	list, _ := f.svc.BlockedRequests(ctx, "7")
	if list[0].Decision != domain.Allowed || list[0].DecidedBy != "Mia" {
		t.Fatalf("decision %+v", list[0])
	}
}

func TestADeskRequestCannotBeAllowedButCanBeDismissed(t *testing.T) {
	f := newFixture(reversingSealer{})
	ctx := context.Background()
	h := hit("api.example.com", "GET")
	h.Proxy = domain.DeskProxy
	if err := f.svc.RecordBlocked(ctx, h); err != nil {
		t.Fatal(err)
	}
	if _, err := f.svc.Allow(ctx, "7", 1, "Mia"); !errors.Is(err, domain.ErrRefused) {
		t.Fatalf("allow of a desk request: %v", err)
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
