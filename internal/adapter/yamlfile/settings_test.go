package yamlfile_test

import (
	"errors"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
)

var exampleSettings = func() []byte {
	b, err := os.ReadFile("testdata/example-settings.yaml")
	if err != nil {
		panic(err)
	}
	return b
}()

func TestParseSettingsExample(t *testing.T) {
	doc, secrets, err := yamlfile.ParseSettings(exampleSettings)
	if err != nil || len(secrets) != 0 {
		t.Fatalf("%v, %v", err, secrets)
	}
	s := doc.Settings()
	app, ok := s.GitHubApp()
	if !ok || app.Name != "github" || *app.App != (domain.GitHubApp{AppID: 12345, PrivateKey: domain.SecretSet, WebhookSecret: domain.SecretSet}) {
		t.Fatalf("github app %+v", app)
	}
	names := make([]string, len(s.Rules))
	for i, r := range s.Rules {
		names[i] = r.Name
	}
	if want := []string{"npm", "anthropic", "github-api", "github-git", "staging-api", "staging-db"}; !reflect.DeepEqual(names, want) {
		t.Fatalf("rules %v, want %v", names, want)
	}
	gh := s.Rules[2]
	wantGH := domain.Rule{
		Name: "github-api", Hosts: []domain.HostPort{{Host: "api.github.com", Port: 443}}, Credential: "github",
		Inject: domain.HeaderInject{Name: "Authorization", Form: "token {secret}"},
		Allow: domain.Access{Entries: []domain.Entry{
			{Methods: []domain.Method{"GET"}, Path: "/repos/{repo}/**"},
			{Methods: []domain.Method{"GET", "POST"}, Path: "/repos/{repo}/pulls"},
			{Methods: []domain.Method{"POST"}, Path: "/graphql"},
		}},
		Ask:    []domain.Entry{{Methods: []domain.Method{"DELETE"}, Path: "/repos/{repo}/**"}},
		BoxEnv: []domain.EnvVar{{Name: "GH_TOKEN", Value: "placeholder"}, {Name: "GH_REPO", Value: "{repo}"}},
	}
	gh.GraphQLMutations = nil
	if !reflect.DeepEqual(gh, wantGH) || len(s.Rules[2].GraphQLMutations) != 13 {
		t.Fatalf("got  %+v\nwant %+v", gh, wantGH)
	}
	if s.Rules[3].Inject != (domain.BasicInject{User: "x-access-token"}) || !s.Rules[5].Allow.Any || s.Rules[5].Hosts[0] != (domain.HostPort{Host: "*.staging.shop.example", Port: 5432}) {
		t.Fatalf("git and db rules %+v %+v", s.Rules[3], s.Rules[5])
	}
}

func TestParseSettingsSecretStates(t *testing.T) {
	cases := []struct {
		value  string
		state  domain.SecretState
		secret string
	}{
		{"set", domain.SecretSet, ""},
		{"not set", domain.SecretNotSet, ""},
		{`"not set"`, domain.SecretNotSet, ""},
		{"sk-live-9f8e7d", domain.SecretSet, "sk-live-9f8e7d"},
		{`"quoted # secret: 1"`, domain.SecretSet, "quoted # secret: 1"},
		{"Set", domain.SecretSet, "Set"},
		{"12345", domain.SecretSet, "12345"},
		{"|\n      line one\n      line two", domain.SecretSet, "line one\nline two\n"},
	}
	for _, c := range cases {
		doc, secrets, err := yamlfile.ParseSettings(yml("credentials:", "  c:", "    value: "+c.value))
		if err != nil {
			t.Errorf("%q: %v", c.value, err)
			continue
		}
		if got := doc.Settings().Credentials[0].Value; got != c.state {
			t.Errorf("%q: state %q, want %q", c.value, got, c.state)
		}
		if want := wantSecret(c.secret); !reflect.DeepEqual(secrets, want) {
			t.Errorf("%q: secrets %v, want %q", c.value, secrets, c.secret)
		}
		assertRenderHolds(t, doc, c.secret)
	}
}

func TestAnAppKeyTypedInPlaceIsLiftedOut(t *testing.T) {
	doc, secrets, err := yamlfile.ParseSettings(yml(
		"credentials:",
		"  github:",
		"    github_app:",
		"      app_id: 7",
		"      private_key: |",
		"        -----BEGIN RSA PRIVATE KEY-----",
		"        c3ludGhldGlj",
		"        -----END RSA PRIVATE KEY-----",
		"      webhook_secret: synthetic-hook-secret",
	))
	if err != nil {
		t.Fatal(err)
	}
	want := []domain.NewSecret{
		{Key: "github/private_key", Value: "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGlj\n-----END RSA PRIVATE KEY-----\n"},
		{Key: "github/webhook_secret", Value: "synthetic-hook-secret"},
	}
	if !reflect.DeepEqual(secrets, want) {
		t.Fatalf("secrets %q", secrets)
	}
	out, _ := doc.Render()
	if strings.Contains(string(out), "c3ludGhldGlj") || strings.Contains(string(out), "synthetic-hook") || strings.Count(string(out), "set") != 2 {
		t.Fatalf("rendered:\n%s", out)
	}
}

func wantSecret(secret string) []domain.NewSecret {
	if secret == "" {
		return []domain.NewSecret{}
	}
	return []domain.NewSecret{{Key: "c/value", Value: secret}}
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
rules: # what the Box reaches
  # the model API
  anthropic:
    hosts: [api.anthropic.com]
    credential: anthropic
    inject: {header: x-api-key, form: "{secret}"}
    allow: "*" # any request
credentials:
  anthropic:
    value: "sk-live-TOPSECRET-1" # sk-live-TOPSECRET-1 is the key
repositories:
  - example-org/shop-web # the shop
# trailing note
`

func TestSettingsRoundTripNeverLeaksASecret(t *testing.T) {
	doc, secrets, err := yamlfile.ParseSettings([]byte(commented))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(secrets, []domain.NewSecret{{Key: "anthropic/value", Value: "sk-live-TOPSECRET-1"}}) {
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
	assertKept(t, text, "# Org settings: keep this file small", "# what the Box reaches", "# the model API", "# any request",
		"# the shop", "# trailing note", "value: set", `form: "{secret}"`)
	assertOrder(t, text, "rules:", "anthropic:", "credentials:", "repositories:")
	again, more, err := yamlfile.ParseSettings(out)
	if err != nil || len(more) != 0 || !reflect.DeepEqual(again.Settings(), doc.Settings()) {
		t.Fatalf("round trip: %v %v", err, more)
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
	again, _, err := yamlfile.ParseSettings(out)
	if err != nil || !reflect.DeepEqual(again.Settings(), doc.Settings()) {
		t.Fatalf("%v:\n%s", err, out)
	}
}

func TestTheOldKeysAreRefusedByName(t *testing.T) {
	_, _, err := yamlfile.ParseSettings(yml("repositories: []", "allow:", "  - a.example:443 GET", "connections: []"))
	var lines yamlfile.ParseErrors
	if !errors.As(err, &lines) || len(lines) != 2 {
		t.Fatalf("err = %v", err)
	}
	if lines[0].Line != 2 || !strings.Contains(lines[0].Msg, "rules") || lines[1].Line != 4 || !strings.Contains(lines[1].Msg, "credentials") {
		t.Fatalf("errors %+v", lines)
	}
}

func TestEachProblemNamesItsLine(t *testing.T) {
	head := []string{"credentials:", "  key:", "    value: set", "rules:", "  r:"}
	cases := []struct {
		name string
		rule []string
		line int
		msg  string
	}{
		{"a bad host", []string{"    hosts: [bad_host]", "    allow: \"*\""}, 6, "host"},
		{"an unknown credential", []string{"    hosts: [a.example]", "    credential: other", "    inject: {basic_user: x}", "    allow: \"*\""}, 7, "not under credentials"},
		{"credential without inject", []string{"    hosts: [a.example]", "    credential: key", "    allow: \"*\""}, 6, "together"},
		{"a bad entry", []string{"    hosts: [a.example]", "    allow:", "      - GET /x", "      - get /y"}, 9, "method"},
		{"a query in an ask entry", []string{"    hosts: [a.example]", "    allow: \"*\"", "    ask: [\"GET /x?y=1\"]"}, 8, "query"},
		{"allow of another word", []string{"    hosts: [a.example]", "    allow: all"}, 7, "allow"},
		{"a form without the secret", []string{"    hosts: [a.example]", "    credential: key", "    inject: {header: X-Key, form: \"Bearer\"}", "    allow: \"*\""}, 8, "{secret}"},
		{"a placeholder without a credential", []string{"    hosts: [a.example]", "    allow: \"*\"", "    box_env: {TOKEN: placeholder}"}, 8, "placeholder"},
		{"a variable the product sets", []string{"    hosts: [a.example]", "    allow: \"*\"", "    box_env: {HTTPS_PROXY: x}"}, 8, "product sets"},
		{"a mutation that is no field name", []string{"    hosts: [a.example]", "    allow: \"*\"", "    graphql_mutations: [\"merge pull\"]"}, 8, "field name"},
		{"an unknown rule key", []string{"    hosts: [a.example]", "    allow: \"*\"", "    deny: [x]"}, 8, "unknown key"},
	}
	for _, c := range cases {
		_, _, err := yamlfile.ParseSettings(yml(append(append([]string{}, head...), c.rule...)...))
		var lines yamlfile.ParseErrors
		if !errors.As(err, &lines) || lines[0].Line != c.line || !strings.Contains(lines[0].Msg, c.msg) {
			t.Errorf("%s: %v, want line %d naming %q", c.name, err, c.line, c.msg)
		}
	}
	_, _, err := yamlfile.ParseSettings(yml("credentials:", "  github:", "    github_app: {app_id: seven, private_key: set}"))
	var lines yamlfile.ParseErrors
	if !errors.As(err, &lines) || lines[0].Line != 3 || !strings.Contains(lines[0].Msg, "app_id") {
		t.Errorf("a bad app id: %v", err)
	}
}
