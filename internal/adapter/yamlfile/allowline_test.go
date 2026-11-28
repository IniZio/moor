package yamlfile_test

import (
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
)

func TestAllowAppendsOneLineAndNamesItsNumber(t *testing.T) {
	doc := "# Settings.\nrepositories:\n  - example-org/shop-web\nallow:\n  - api.example.com:443 GET # the API\nconnections: []\n"
	out, line, err := yamlfile.AppendAllow([]byte(doc), "fonts.example.com:443 GET")
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(string(out), "\n")
	if line < 1 || lines[line-1] != "  - fonts.example.com:443 GET" {
		t.Fatalf("line %d of:\n%s", line, out)
	}
	if !strings.Contains(string(out), "# the API") || !strings.Contains(string(out), "# Settings.") {
		t.Errorf("the comments were lost:\n%s", out)
	}
	again, line2, err := yamlfile.AppendAllow(out, "fonts.example.com:443 GET")
	if err != nil || string(again) != string(out) || line2 != line {
		t.Errorf("a second allow of the same line changed the document: %v line %d:\n%s", err, line2, again)
	}
}

func TestAllowFillsAnEmptyOrMissingAllowList(t *testing.T) {
	for name, doc := range map[string]string{
		"flow":    "repositories: []\nallow: []\nconnections: []\n",
		"missing": "repositories: []\n",
		"empty":   "",
	} {
		out, line, err := yamlfile.AppendAllow([]byte(doc), "api.example.com:443 POST")
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		doc2, _, err := yamlfile.ParseSettings(out)
		if err != nil {
			t.Fatalf("%s: the result does not parse: %v\n%s", name, err, out)
		}
		allow := doc2.Settings().Allow
		if len(allow) != 1 || allow[0].String() != "api.example.com:443 POST" || line < 1 {
			t.Errorf("%s: allow %v at line %d:\n%s", name, allow, line, out)
		}
	}
}

func TestAllowRefusesABadRuleOrADocumentWithAValue(t *testing.T) {
	if _, _, err := yamlfile.AppendAllow([]byte("allow: []\n"), "not a rule"); err == nil {
		t.Error("a bad rule was appended")
	}
	typed := "connections:\n  - name: K\n    hosts: [api.example.com]\n    add: placeholder K\n    value: s3cr3t-typed\n"
	if _, _, err := yamlfile.AppendAllow([]byte(typed), "api.example.com:443 GET"); err == nil {
		t.Error("a document holding a typed value was rendered")
	}
}
