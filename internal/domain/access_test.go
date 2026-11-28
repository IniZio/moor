package domain_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func rule(t *testing.T, s string) domain.AllowRule {
	t.Helper()
	r, err := domain.ParseAllowRule(s)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func TestAPolicyPassesOnlyAListedHostPortAndMethod(t *testing.T) {
	p := domain.Policy{Allow: []domain.AllowRule{rule(t, "api.example.com:443 GET POST"), rule(t, "*.cdn.example.com:443 GET")}}
	cases := []struct {
		host   string
		port   int
		method domain.Method
		reason domain.DenyReason
	}{
		{"api.example.com", 443, "GET", ""},
		{"API.example.com", 443, "POST", ""},
		{"a.b.cdn.example.com", 443, "GET", ""},
		{"api.example.com", 443, "DELETE", domain.DenyMethod},
		{"api.example.com", 80, "GET", domain.DenyHost},
		{"cdn.example.com", 443, "GET", domain.DenyHost},
		{"other.example.com", 443, "GET", domain.DenyHost},
		{"*.cdn.example.com", 443, "GET", domain.DenyHost},
	}
	for _, c := range cases {
		reason, ok := p.Check(c.host, c.port, c.method)
		if reason != c.reason || ok != (c.reason == "") {
			t.Errorf("%s:%d %s: got %q %v, want %q", c.host, c.port, c.method, reason, ok, c.reason)
		}
	}
	if !p.Tunnel("api.example.com", 443) || p.Tunnel("api.example.com", 80) || p.Tunnel("other.example.com", 443) {
		t.Error("a tunnel follows the host and port of the rules")
	}
	if _, ok := (domain.Policy{}).Check("api.example.com", 443, "GET"); ok {
		t.Error("an empty policy passed a request")
	}
}

func TestTheBoxProxyGetsTheRulesAndTheDeskProxyTheModelConnections(t *testing.T) {
	model := domain.Connection{Name: "Model key", Hosts: []domain.HostPattern{"api.anthropic.com"}, Add: domain.HeaderAdd{Name: "x-api-key", Template: "{value}"}, Secret: domain.SecretSet}
	staging := domain.Connection{Name: "Staging key", Hosts: []domain.HostPattern{"api.example.com"}, Add: domain.PlaceholderAdd{Token: "STAGING_KEY"}, Secret: domain.SecretSet}
	s := domain.Settings{Allow: []domain.AllowRule{rule(t, "api.example.com:443 GET")}, Connections: []domain.Connection{model, staging}}
	portal := domain.AnyMethod("portal.example.com", 443)

	boxGetsRules(t, s.PolicyFor(domain.BoxProxy, 7, portal))
	deskGetsModel(t, s.PolicyFor(domain.DeskProxy, 7, portal))
}

func boxGetsRules(t *testing.T, box domain.Policy) {
	t.Helper()
	if box.Version != 7 || len(box.Connections) != 1 || box.Connections[0].Name != "Staging key" {
		t.Fatalf("box policy %+v", box)
	}
	if _, ok := box.Check("api.anthropic.com", 443, "POST"); ok {
		t.Error("the Box reaches the model host")
	}
	if _, ok := box.Check("portal.example.com", 443, "GET"); ok {
		t.Error("the Box reaches the portal")
	}
}

func deskGetsModel(t *testing.T, desk domain.Policy) {
	t.Helper()
	if len(desk.Connections) != 1 || desk.Connections[0].Name != "Model key" {
		t.Fatalf("desk connections %+v", desk.Connections)
	}
	for _, host := range []string{"api.anthropic.com", "portal.example.com"} {
		if _, ok := desk.Check(host, 443, "POST"); !ok {
			t.Errorf("the desk cannot reach %s", host)
		}
	}
	if _, ok := desk.Check("api.example.com", 443, "GET"); ok {
		t.Error("the desk reaches a host of the Box's allow rules")
	}
	if c, ok := desk.ConnectionFor("API.anthropic.com"); !ok || c.Name != "Model key" {
		t.Errorf("connection for the model host: %+v %v", c, ok)
	}
}

func TestABlockedBoxRequestTurnsIntoOneAllowLine(t *testing.T) {
	b := domain.BlockedRequest{Proxy: domain.BoxProxy, Host: "fonts.example.com", Port: 443, Method: "GET"}
	if b.Request() != "fonts.example.com:443 GET" || !b.Allowable() {
		t.Fatalf("%q allowable %v", b.Request(), b.Allowable())
	}
	b.Proxy = domain.DeskProxy
	if b.Allowable() {
		t.Error("a desk request is allowable")
	}
	odd := domain.BlockedRequest{Proxy: domain.BoxProxy, Host: "x.example.com", Port: 443, Method: "PROPFIND"}
	if odd.Allowable() {
		t.Error("a method no rule can name is allowable")
	}
}

func TestABlockedReportIsCheckedBeforeItIsStored(t *testing.T) {
	good := domain.BlockedHit{Task: "1", Proxy: domain.BoxProxy, Host: "api.example.com", Port: 443, Method: "GET", Reason: domain.DenyHost}
	if err := good.Validate(); err != nil {
		t.Fatal(err)
	}
	for name, mut := range map[string]func(*domain.BlockedHit){
		"wildcard": func(h *domain.BlockedHit) { h.Host = "*.example.com" },
		"proxy":    func(h *domain.BlockedHit) { h.Proxy = "other" },
		"port":     func(h *domain.BlockedHit) { h.Port = 0 },
		"reason":   func(h *domain.BlockedHit) { h.Reason = "because" },
		"method":   func(h *domain.BlockedHit) { h.Method = "get" },
	} {
		h := good
		mut(&h)
		if h.Validate() == nil {
			t.Errorf("%s: a bad report passed", name)
		}
	}
}
