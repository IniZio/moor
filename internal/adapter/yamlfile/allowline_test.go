package yamlfile_test

import (
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
)

func allowEntry(t *testing.T, doc string, req yamlfile.AllowRequest) (string, int) {
	t.Helper()
	out, line, err := yamlfile.AllowEntry([]byte(doc), req)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(string(out), "\n")
	if line < 1 || line > len(lines) || !strings.Contains(lines[line-1], strings.TrimSuffix(req.Entry, "*")) {
		t.Fatalf("line %d does not hold %q:\n%s", line, req.Entry, out)
	}
	return string(out), line
}

func ruleOf(t *testing.T, doc, name string) domain.Rule {
	t.Helper()
	parsed, _, err := yamlfile.ParseSettings([]byte(doc))
	if err != nil {
		t.Fatalf("%v:\n%s", err, doc)
	}
	for _, r := range parsed.Settings().Rules {
		if r.Name == name {
			return r
		}
	}
	t.Fatalf("no rule %s in:\n%s", name, doc)
	return domain.Rule{}
}

const ruleDoc = `# Settings.
rules:
  api:
    hosts: [api.example.com] # the API
    allow: ["GET /items/**"]
    ask:
      - "DELETE /items/*"
`

func TestAllowAppendsTheEntryToTheRuleAndNamesItsLine(t *testing.T) {
	out, line := allowEntry(t, ruleDoc, yamlfile.AllowRequest{Rule: "api", Entry: "POST /items"})
	if got := ruleOf(t, out, "api").Allow.Entries; len(got) != 2 || got[1].String() != "POST /items" {
		t.Fatalf("allow %v:\n%s", got, out)
	}
	if !strings.Contains(out, "# the API") || !strings.Contains(out, "# Settings.") {
		t.Errorf("the comments were lost:\n%s", out)
	}
	again, line2 := allowEntry(t, out, yamlfile.AllowRequest{Rule: "api", Entry: "POST /items"})
	if again != out || line2 != line {
		t.Errorf("a second allow of the same entry changed the document, line %d:\n%s", line2, again)
	}
}

func TestAllowMovesAnAskEntryToAllow(t *testing.T) {
	out, _ := allowEntry(t, ruleDoc, yamlfile.AllowRequest{Rule: "api", Entry: "DELETE /items/*"})
	r := ruleOf(t, out, "api")
	if len(r.Ask) != 0 || len(r.Allow.Entries) != 2 || r.Allow.Entries[1].String() != "DELETE /items/*" || strings.Contains(out, "ask") {
		t.Fatalf("rule %+v:\n%s", r, out)
	}
}

func TestAllowAddsARuleForANewHost(t *testing.T) {
	for name, doc := range map[string]string{"with rules": ruleDoc, "missing": "repositories: []\n", "empty": ""} {
		out, _ := allowEntry(t, doc, yamlfile.AllowRequest{Entry: "GET /x", Host: domain.HostPort{Host: "later.example", Port: 80}})
		r := ruleOf(t, out, "later.example-80")
		want := []domain.HostPort{{Host: "later.example", Port: 80}}
		if !reflect.DeepEqual(r.Hosts, want) || len(r.Allow.Entries) != 1 || r.Allow.Entries[0].String() != "GET /x" {
			t.Fatalf("%s: rule %+v:\n%s", name, r, out)
		}
	}
	out, _ := allowEntry(t, ruleDoc, yamlfile.AllowRequest{Entry: "*", Host: domain.HostPort{Host: "api.example.com", Port: 443}})
	if r := ruleOf(t, out, "api.example.com"); !r.Allow.Any {
		t.Fatalf("a refused tunnel's rule %+v:\n%s", r, out)
	}
	out, _ = allowEntry(t, out, yamlfile.AllowRequest{Entry: "*", Host: domain.HostPort{Host: "api.example.com", Port: 443}})
	ruleOf(t, out, "api.example.com-2")
}

func TestAllowRefusesAnEntryNoRuleCouldHold(t *testing.T) {
	for _, req := range []yamlfile.AllowRequest{{Rule: "api", Entry: "GET /x?y=1"}, {Rule: "api", Entry: "*"}} {
		if _, _, err := yamlfile.AllowEntry([]byte(ruleDoc), req); err == nil {
			t.Errorf("%+v was added", req)
		}
	}
}
