package yamlfile_test

import (
	"errors"
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
)

var exampleSettings = yml(
	"repositories:",
	"  - example-org/shop-web",
	"allow:",
	"  - registry.npmjs.org:443 GET",
	"  - staging-api.shop.example:443 GET POST",
	"connections:",
	"  - name: Anthropic API",
	"    hosts: [api.anthropic.com]",
	`    add: header x-api-key "{value}"`,
	"    value: set",
)

func TestParseSettingsExample(t *testing.T) {
	doc, secrets, err := yamlfile.ParseSettings(exampleSettings)
	if err != nil || len(secrets) != 0 {
		t.Fatalf("%v, %v", err, secrets)
	}
	want := domain.Settings{
		Repositories: []domain.Repo{{Host: "github.com", Path: "example-org/shop-web"}},
		Allow: []domain.AllowRule{
			{Host: "registry.npmjs.org", Port: 443, Methods: []domain.Method{"GET"}},
			{Host: "staging-api.shop.example", Port: 443, Methods: []domain.Method{"GET", "POST"}},
		},
		Connections: []domain.Connection{{
			Name: "Anthropic API", Hosts: []domain.HostPattern{"api.anthropic.com"},
			Add: domain.HeaderAdd{Name: "x-api-key", Template: "{value}"}, Secret: domain.SecretSet,
		}},
	}
	if got := doc.Settings(); !reflect.DeepEqual(got, want) {
		t.Fatalf("got  %+v\nwant %+v", got, want)
	}
	uncovered := doc.Settings().UncoveredHosts()
	if len(uncovered) != 1 || uncovered[0].Host != "api.anthropic.com" || uncovered[0].Connection != "Anthropic API" {
		t.Fatalf("uncovered = %v", uncovered)
	}
}

func TestParseSettingsSecretStates(t *testing.T) {
	conn := func(value string) []byte {
		return yml("connections:", "  - name: C", "    hosts: [a.example]", "    add: placeholder TOKEN", "    value: "+value)
	}
	cases := []struct {
		value  string
		state  domain.SecretState
		secret string
	}{
		{"set", domain.SecretSet, ""},
		{"not set", domain.SecretNotSet, ""},
		{`"not set"`, domain.SecretNotSet, ""},
		{`'set'`, domain.SecretSet, ""},
		{"sk-live-9f8e7d", domain.SecretSet, "sk-live-9f8e7d"},
		{`"quoted # secret: 1"`, domain.SecretSet, "quoted # secret: 1"},
		{"Set", domain.SecretSet, "Set"},
		{"12345", domain.SecretSet, "12345"},
		{"true", domain.SecretSet, "true"},
		{`"set "`, domain.SecretSet, "set "},
		{"|\n      line one\n      line two", domain.SecretSet, "line one\nline two\n"},
	}
	for _, c := range cases {
		doc, secrets, err := yamlfile.ParseSettings(conn(c.value))
		if err != nil {
			t.Errorf("%q: %v", c.value, err)
			continue
		}
		if got := doc.Settings().Connections[0].Secret; got != c.state {
			t.Errorf("%q: state %q, want %q", c.value, got, c.state)
		}
		wantSecrets := []domain.NewSecret(nil)
		if c.secret != "" {
			wantSecrets = []domain.NewSecret{{Connection: "C", Value: c.secret}}
		}
		if !reflect.DeepEqual(secrets, wantSecrets) {
			t.Errorf("%q: secrets %v, want %q", c.value, secrets, c.secret)
		}
		assertRenderHolds(t, doc, c.secret)
	}
}

func assertRenderHolds(t *testing.T, doc yamlfile.SettingsDoc, secret string) {
	t.Helper()
	out, err := doc.Render()
	if err != nil || secret != "" && containsAny(string(out), strings.Split(secret, "\n")) {
		t.Errorf("rendered %q, %v", out, err)
	}
}

func containsAny(text string, parts []string) bool {
	for _, p := range parts {
		if p != "" && strings.Contains(text, p) {
			return true
		}
	}
	return false
}

const commented = `# Org settings: keep this file small
connections: # who we hold secrets for
  # the model API
  - name: Anthropic API
    hosts: [api.anthropic.com]
    add: header x-api-key "{value}"
    value: "sk-live-TOPSECRET-1" # sk-live-TOPSECRET-1 is the key
  - name: Other
    hosts: [b.example, "*.c.example"]
    add: placeholder OTHER_TOKEN
    value: not set
allow:
  - api.anthropic.com:443 POST # the API
  - "*.c.example:443 GET"
repositories:
  - example-org/shop-web # the shop
# trailing note
`

func TestSettingsRoundTripNeverLeaksASecret(t *testing.T) {
	doc, secrets, err := yamlfile.ParseSettings([]byte(commented))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(secrets, []domain.NewSecret{{Connection: "Anthropic API", Value: "sk-live-TOPSECRET-1"}}) {
		t.Fatalf("secrets = %v", secrets)
	}
	out, err := doc.Render()
	if err != nil {
		t.Fatal(err)
	}
	text := string(out)
	if strings.Contains(text, "TOPSECRET") || strings.Contains(text, "sk-live") {
		t.Fatalf("the secret leaked:\n%s", text)
	}
	assertKept(t, text, "# Org settings: keep this file small", "# who we hold secrets for", "# the model API", "# the API",
		"# the shop", "# trailing note", "value: set", "value: not set", `add: header x-api-key "{value}"`, "*.c.example")
	assertOrder(t, text, "connections:", "name: Anthropic API", "name: Other", "allow:", "repositories:")
	again, secrets2, err := yamlfile.ParseSettings(out)
	if err != nil || len(secrets2) != 0 {
		t.Fatalf("reparse: %v, %v", err, secrets2)
	}
	if !reflect.DeepEqual(again.Settings(), doc.Settings()) {
		t.Fatalf("round trip changed the document:\n%+v\n%+v", again.Settings(), doc.Settings())
	}
	out2, err := again.Render()
	if err != nil || string(out2) != text {
		t.Fatalf("render is not stable:\n%s\n%s", out2, text)
	}
}

func assertOrder(t *testing.T, text string, keys ...string) {
	t.Helper()
	last := -1
	for _, k := range keys {
		at := strings.Index(text, k)
		if at <= last {
			t.Fatalf("key order changed at %q:\n%s", k, text)
		}
		last = at
	}
}

func assertKept(t *testing.T, text string, parts ...string) {
	t.Helper()
	for _, p := range parts {
		if !strings.Contains(text, p) {
			t.Errorf("render lost %q:\n%s", p, text)
		}
	}
}

func TestSettingsFromTypedValuesRender(t *testing.T) {
	doc, _, err := yamlfile.ParseSettings(exampleSettings)
	if err != nil {
		t.Fatal(err)
	}
	out, err := yamlfile.RenderSettings(doc.Settings())
	if err != nil {
		t.Fatal(err)
	}
	again, secrets, err := yamlfile.ParseSettings(out)
	if err != nil || len(secrets) != 0 || !reflect.DeepEqual(again.Settings(), doc.Settings()) {
		t.Fatalf("%v %v\n%s", err, secrets, out)
	}
	empty, err := yamlfile.RenderSettings(domain.Settings{})
	if err != nil {
		t.Fatal(err)
	}
	back, _, err := yamlfile.ParseSettings(empty)
	if err != nil || !reflect.DeepEqual(back.Settings(), domain.Settings{}) {
		t.Fatalf("%q: %+v, %v", empty, back.Settings(), err)
	}
}

func TestParseSettingsEmptyDocument(t *testing.T) {
	for _, in := range []string{"", "\n", "# only a comment\n", "---\n", "{}"} {
		doc, secrets, err := yamlfile.ParseSettings([]byte(in))
		if err != nil || len(secrets) != 0 || !reflect.DeepEqual(doc.Settings(), domain.Settings{}) {
			t.Errorf("%q: %+v %v %v", in, doc.Settings(), secrets, err)
		}
		if _, err := doc.Render(); err != nil {
			t.Errorf("%q: render: %v", in, err)
		}
	}
}

func conn(extra ...string) []byte {
	return yml(append([]string{"connections:"}, extra...)...)
}

func TestParseSettingsErrors(t *testing.T) {
	good := []string{"  - name: C", "    hosts: [a.example]", "    add: placeholder T", "    value: set"}
	cases := []struct {
		name string
		in   []byte
		line int
		msg  string
	}{
		{"unknown top key", yml("repositories: []", "secrets: []"), 2, `unknown key "secrets"`},
		{"unknown connection key", conn("  - name: C", "    hosts: [a.example]", "    add: placeholder T", "    value: set", "    extra: 1"), 6, `unknown key "extra"`},
		{"top level is a list", yml("- a"), 1, "must be a mapping"},
		{"repositories is text", yml("repositories: a/b"), 1, "must be a list"},
		{"repository is bad", yml("repositories:", "  - a/b", "  - nope"), 3, "nope"},
		{"repository twice", yml("repositories:", "  - a/b", "  - github.com/A/B"), 3, "listed twice"},
		{"repository is a number", yml("repositories: [1]"), 1, "must be text"},
		{"allow is a map", yml("allow: {a: b}"), 1, "must be a list"},
		{"allow rule without a method", yml("allow:", "  - a.example:443"), 2, "at least one method"},
		{"allow rule without a port", yml("allow:", "  - a.example GET"), 2, "host:port"},
		{"allow rule with a bad port", yml("allow:", "  - a.example:99999 GET"), 2, "port"},
		{"allow rule with a lower-case method", yml("allow:", "  - a.example:443 get"), 2, "method"},
		{"allow rule with a repeated method", yml("allow:", "  - a.example:443 GET GET"), 2, "method"},
		{"allow rule is a number", yml("allow:", "  - 5"), 2, "must be text"},
		{"second allow rule is bad", yml("allow:", "  - a.example:443 GET", "  - b.example:x GET"), 3, "b.example:x"},
		{"connections is a map", yml("connections: {a: b}"), 1, "must be a list"},
		{"connection is text", conn("  - hello"), 2, "must be a mapping"},
		{"connection without a name", conn("  - hosts: [a.example]", "    add: placeholder T", "    value: set"), 2, `missing required key "name"`},
		{"connection without hosts", conn("  - name: C", "    add: placeholder T", "    value: set"), 2, `missing required key "hosts"`},
		{"connection without add", conn("  - name: C", "    hosts: [a.example]", "    value: set"), 2, `missing required key "add"`},
		{"connection without value", conn("  - name: C", "    hosts: [a.example]", "    add: placeholder T"), 2, `missing required key "value"`},
		{"empty name", conn("  - name: ''", "    hosts: [a.example]", "    add: placeholder T", "    value: set"), 2, "name is empty"},
		{"name is a number", conn("  - name: 5", "    hosts: [a.example]", "    add: placeholder T", "    value: set"), 2, "name must be text"},
		{"two connections without names", conn("  - 0: a", "  - 0: b"), 2, "unknown key"},
		{"name used twice", conn(append(append(append([]string{}, good...), "  - name: C", "    hosts: [b.example]"), "    add: placeholder T", "    value: set")...), 6, "used twice"},
		{"hosts is text", conn("  - name: C", "    hosts: a.example", "    add: placeholder T", "    value: set"), 3, "must be a list"},
		{"hosts is empty", conn("  - name: C", "    hosts: []", "    add: placeholder T", "    value: set"), 3, "at least one host"},
		{"host with a port", conn("  - name: C", "    hosts: [a.example:443]", "    add: placeholder T", "    value: set"), 3, "a.example:443"},
		{"host with a scheme", conn("  - name: C", "    hosts: ['https://a.example']", "    add: placeholder T", "    value: set"), 3, "https"},
		{"host twice", conn("  - name: C", "    hosts: [a.example, A.example]", "    add: placeholder T", "    value: set"), 3, "listed twice"},
		{"bad wildcard", conn("  - name: C", "    hosts: ['*']", "    add: placeholder T", "    value: set"), 3, "*"},
		{"add without a mark", conn("  - name: C", "    hosts: [a.example]", "    add: header X \"y\"", "    value: set"), 4, "{value}"},
		{"add of another kind", conn("  - name: C", "    hosts: [a.example]", "    add: cookie T", "    value: set"), 4, "cookie"},
		{"add is a list", conn("  - name: C", "    hosts: [a.example]", "    add: [a]", "    value: set"), 4, "add must be text"},
		{"value is empty", conn("  - name: C", "    hosts: [a.example]", "    add: placeholder T", "    value: ''"), 5, "value must be"},
		{"value is null", conn("  - name: C", "    hosts: [a.example]", "    add: placeholder T", "    value:"), 5, "value must be"},
		{"value is a list", conn("  - name: C", "    hosts: [a.example]", "    add: placeholder T", "    value: [a]"), 5, "value must be"},
		{"two documents", yml("repositories: []", "---", "allow: []"), 2, "second YAML document"},
		{"alias", yml("repositories: &r [a/b]", "allow: *r"), 2, "aliases"},
		{"syntax", yml("repositories: [a/b"), 1, "expected"},
	}
	for _, c := range cases {
		_, secrets, err := yamlfile.ParseSettings(c.in)
		var pe yamlfile.ParseErrors
		if !errors.As(err, &pe) || !errors.Is(err, yamlfile.ErrInvalid) || len(pe) == 0 || len(secrets) != 0 {
			t.Errorf("%s: err = %v, secrets %v", c.name, err, secrets)
			continue
		}
		if pe[0].Line != c.line || !strings.Contains(pe[0].Msg, c.msg) {
			t.Errorf("%s: got line %d %q, want line %d containing %q", c.name, pe[0].Line, pe[0].Msg, c.line, c.msg)
		}
	}
}

func TestSettingsErrorsNeverQuoteASecret(t *testing.T) {
	in := yml(
		"connections:",
		"  - name: C",
		"    hosts: [bad host]",
		"    add: placeholder T",
		"    value: sk-live-TOPSECRET-2",
		"    extra: sk-live-TOPSECRET-2",
		"  - name: C",
		"    hosts: [a.example]",
		"    add: header X sk-live-TOPSECRET-2",
		"    value: sk-live-TOPSECRET-3",
	)
	_, secrets, err := yamlfile.ParseSettings(in)
	var pe yamlfile.ParseErrors
	if !errors.As(err, &pe) || len(pe) < 3 || len(secrets) != 0 {
		t.Fatalf("err = %v, secrets %v", err, secrets)
	}
	if strings.Contains(err.Error(), "TOPSECRET-2") && !strings.Contains(err.Error(), "header X sk-live") {
		t.Fatalf("an error quotes a secret value: %v", err)
	}
	if strings.Contains(err.Error(), "TOPSECRET-3") {
		t.Fatalf("an error quotes a secret value: %v", err)
	}
}
