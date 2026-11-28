package domain_test

import (
	"errors"
	"reflect"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestParseAllowRule(t *testing.T) {
	ok := map[string]domain.AllowRule{
		"registry.npmjs.org:443 GET":                {Host: "registry.npmjs.org", Port: 443, Methods: []domain.Method{"GET"}},
		"  staging-api.shop.example:443  GET POST ": {Host: "staging-api.shop.example", Port: 443, Methods: []domain.Method{"GET", "POST"}},
		"*.Example.com:8080 DELETE PATCH":           {Host: "*.example.com", Port: 8080, Methods: []domain.Method{"DELETE", "PATCH"}},
		"localhost:1 HEAD OPTIONS PUT":              {Host: "localhost", Port: 1, Methods: []domain.Method{"HEAD", "OPTIONS", "PUT"}},
		"10.0.0.1:65535 GET":                        {Host: "10.0.0.1", Port: 65535, Methods: []domain.Method{"GET"}},
	}
	for in, want := range ok {
		got, err := domain.ParseAllowRule(in)
		if err != nil || !reflect.DeepEqual(got, want) {
			t.Errorf("%q: got %+v, %v, want %+v", in, got, err, want)
		}
	}
	for _, in := range []string{
		"", "example.com:443", "example.com GET", "example.com:443GET", ":443 GET", "example.com: GET", "example.com:0 GET",
		"example.com:65536 GET", "example.com:-1 GET", "example.com:http GET", "example.com:443 get", "example.com:443 GET GET",
		"example.com:443 FETCH", "example.com:443 *", "*:443 GET", "*.:443 GET", "a*.example.com:443 GET", "ex ample.com:443 GET",
		"-a.com:443 GET", "a..com:443 GET", "https://example.com:443 GET", "example.com/path:443 GET",
	} {
		if _, err := domain.ParseAllowRule(in); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("%q: err = %v", in, err)
		}
	}
	rule, _ := domain.ParseAllowRule("a.example:80 GET POST")
	if rule.String() != "a.example:80 GET POST" {
		t.Errorf("String = %q", rule.String())
	}
}

func TestParseAddRule(t *testing.T) {
	ok := map[string]domain.AddRule{
		`header x-api-key "{value}"`:            domain.HeaderAdd{Name: "x-api-key", Template: "{value}"},
		`header Authorization "Bearer {value}"`: domain.HeaderAdd{Name: "Authorization", Template: "Bearer {value}"},
		`  header X-A   "a {value} b {value}" `: domain.HeaderAdd{Name: "X-A", Template: "a {value} b {value}"},
		`header X "say \"{value}\""`:            domain.HeaderAdd{Name: "X", Template: `say "{value}"`},
		"placeholder MOOR_ANTHROPIC_KEY":        domain.PlaceholderAdd{Token: "MOOR_ANTHROPIC_KEY"},
		"placeholder  token1 ":                  domain.PlaceholderAdd{Token: "token1"},
	}
	for in, want := range ok {
		got, err := domain.ParseAddRule(in)
		if err != nil || got != want {
			t.Errorf("%q: got %#v, %v, want %#v", in, got, err, want)
		}
		if again, err := domain.ParseAddRule(got.String()); err != nil || again != want {
			t.Errorf("%q: String %q does not parse back: %v", in, got.String(), err)
		}
	}
	for _, in := range []string{
		"", "header", "header x-api-key", `header x-api-key {value}`, `header x-api-key "no mark"`, `header x-api-key "{value}" extra`,
		`header x-api-key 'x {value}'`, `header x-api-key "{value}`, `header bad_name "{value}"`, `header "x" "{value}"`,
		`header x "{value}\r\nX-Evil: 1"`, `header x "a\x00{value}"`, "placeholder", "placeholder 1ABC", "placeholder A B", "placeholder A-B",
		"cookie x {value}", "Header x \"{value}\"", `header x-api-key "{Value}"`,
	} {
		if _, err := domain.ParseAddRule(in); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("%q: err = %v", in, err)
		}
	}
}

func TestHostPatternCovers(t *testing.T) {
	cases := []struct {
		rule, host string
		want       bool
	}{
		{"api.example.com", "api.example.com", true},
		{"api.example.com", "other.example.com", false},
		{"api.example.com", "*.example.com", false},
		{"*.example.com", "api.example.com", true},
		{"*.example.com", "a.b.example.com", true},
		{"*.example.com", "example.com", false},
		{"*.example.com", "badexample.com", false},
		{"*.example.com", "*.example.com", true},
		{"*.example.com", "*.api.example.com", true},
		{"*.api.example.com", "*.example.com", false},
		{"*.example.com", "*.other.com", false},
	}
	for _, c := range cases {
		rule, err1 := domain.ParseHostPattern(c.rule)
		host, err2 := domain.ParseHostPattern(c.host)
		if err1 != nil || err2 != nil {
			t.Fatalf("%v %v", err1, err2)
		}
		if got := rule.Covers(host); got != c.want {
			t.Errorf("%s covers %s = %v, want %v", c.rule, c.host, got, c.want)
		}
	}
}

func TestUncoveredHostsAndValidate(t *testing.T) {
	allow := func(s string) domain.AllowRule {
		r, err := domain.ParseAllowRule(s)
		if err != nil {
			t.Fatal(err)
		}
		return r
	}
	conn := func(name string, hosts ...domain.HostPattern) domain.Connection {
		return domain.Connection{Name: name, Hosts: hosts, Add: domain.PlaceholderAdd{Token: "T"}, Secret: domain.SecretSet}
	}
	s := domain.Settings{
		Repositories: []domain.Repo{shopWeb},
		Allow:        []domain.AllowRule{allow("api.anthropic.com:443 POST"), allow("*.shop.example:443 GET")},
		Connections: []domain.Connection{
			conn("Anthropic", "api.anthropic.com"),
			conn("Shop", "a.shop.example", "b.other.example", "*.shop.example", "*.example"),
		},
	}
	if err := s.Validate(); err != nil {
		t.Fatal(err)
	}
	want := []domain.UncoveredHost{{Connection: "Shop", Host: "b.other.example"}, {Connection: "Shop", Host: "*.example"}}
	if got := s.UncoveredHosts(); !reflect.DeepEqual(got, want) {
		t.Errorf("uncovered = %v, want %v", got, want)
	}
	if (domain.Settings{}).UncoveredHosts() != nil {
		t.Error("empty settings have uncovered hosts")
	}
	for name, mutate := range map[string]func(*domain.Settings){
		"repeated repository":   func(s *domain.Settings) { s.Repositories = append(s.Repositories, shopWeb) },
		"incomplete repository": func(s *domain.Settings) { s.Repositories = []domain.Repo{{Path: "a/b"}} },
		"repeated name":         func(s *domain.Settings) { s.Connections = append(s.Connections, conn("Shop", "x.example")) },
		"no name":               func(s *domain.Settings) { s.Connections[0].Name = "" },
		"no hosts":              func(s *domain.Settings) { s.Connections[0].Hosts = nil },
		"no add rule":           func(s *domain.Settings) { s.Connections[0].Add = nil },
		"unknown secret":        func(s *domain.Settings) { s.Connections[0].Secret = "hunter2" },
	} {
		c := s
		c.Connections = append([]domain.Connection(nil), s.Connections...)
		c.Connections[0].Hosts = append([]domain.HostPattern(nil), s.Connections[0].Hosts...)
		mutate(&c)
		if err := c.Validate(); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
}
