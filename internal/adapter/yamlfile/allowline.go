package yamlfile

import (
	"fmt"
	"strings"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

// AllowRequest is what Allow on a blocked request adds: Entry to the allow list of Rule, or, with no Rule, a new
// rule for Host. An Entry of * makes the new rule allow every request, as a refused tunnel needs.
type AllowRequest struct {
	Rule  string
	Entry string
	Host  domain.HostPort
}

// AllowEntry adds the entry to the document, keeping the rest of it as it is. An entry the rule holds under ask
// moves to allow. An entry already allowed is kept as it is. It returns the document and the line of the entry.
func AllowEntry(text []byte, req AllowRequest) ([]byte, int, error) {
	doc, err := allowable(text, req.Entry)
	if err != nil {
		return nil, 0, err
	}
	rules := child(doc.src.Content[0], "rules", mapping)
	rules.Style = 0
	name := req.Rule
	if r := value(rules, name); name != "" && r != nil {
		if err := allowIn(r, req.Entry); err != nil {
			return nil, 0, err
		}
	} else {
		name = freeName(rules, strings.ReplaceAll(req.Host.String(), ":", "-"))
		rules.Content = append(rules.Content, str(name), ruleNode(newRule(req)))
	}
	out, err := doc.Render()
	if err != nil {
		return nil, 0, err
	}
	if _, _, err := ParseSettings(out); err != nil {
		return nil, 0, fmt.Errorf("%w: the document with the entry does not read back: %w", ErrInvalid, err)
	}
	line, err := entryLine(out, name, req.Entry)
	return out, line, err
}

// allowable is the stored document the entry goes into, after checking both.
func allowable(text []byte, entry string) (SettingsDoc, error) {
	if entry != domain.AnyText {
		if _, err := domain.ParseEntry(entry); err != nil {
			return SettingsDoc{}, fmt.Errorf("%w: %w", ErrInvalid, err)
		}
	}
	doc, secrets, err := ParseSettings(text)
	switch {
	case err != nil:
		return SettingsDoc{}, err
	case len(secrets) > 0:
		return SettingsDoc{}, fmt.Errorf("%w: the document holds a typed secret", ErrInvalid)
	case doc.src == nil:
		doc.src = &yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{mapping()}}
	}
	return doc, nil
}

func newRule(req AllowRequest) domain.Rule {
	r := domain.Rule{Hosts: []domain.HostPort{req.Host}, Allow: domain.Access{Any: true}}
	if req.Entry != domain.AnyText {
		e, _ := domain.ParseEntry(req.Entry)
		r.Allow = domain.Access{Entries: []domain.Entry{e}}
	}
	return r
}

// allowIn moves entry from the rule's ask list to its allow list, or appends it to allow.
func allowIn(rule *yaml.Node, entry string) error {
	if entry == domain.AnyText {
		return fmt.Errorf("%w: * is for a new rule only", ErrInvalid)
	}
	if ask := value(rule, "ask"); ask != nil {
		ask.Content = without(ask.Content, entry)
		if len(ask.Content) == 0 {
			drop(rule, "ask")
		}
	}
	allow := value(rule, "allow")
	if allow.Kind != yaml.SequenceNode || hasItem(allow, entry) {
		return nil
	}
	allow.Style = 0
	allow.Content = append(allow.Content, str(entry))
	return nil
}

// child is the value of key in m, made with mk and added when m has none.
func child(m *yaml.Node, key string, mk func(...*yaml.Node) *yaml.Node) *yaml.Node {
	if v := value(m, key); v != nil && v.Kind == yaml.MappingNode {
		return v
	}
	drop(m, key)
	v := mk()
	m.Content = append(m.Content, str(key), v)
	return v
}

func value(m *yaml.Node, key string) *yaml.Node {
	for i := 0; i+1 < len(m.Content); i += 2 {
		if m.Content[i].Value == key {
			return m.Content[i+1]
		}
	}
	return nil
}

func drop(m *yaml.Node, key string) {
	for i := 0; i+1 < len(m.Content); i += 2 {
		if m.Content[i].Value == key {
			m.Content = append(m.Content[:i:i], m.Content[i+2:]...)
			return
		}
	}
}

func without(items []*yaml.Node, text string) []*yaml.Node {
	var out []*yaml.Node
	for _, n := range items {
		if n.Value != text {
			out = append(out, n)
		}
	}
	return out
}

func hasItem(list *yaml.Node, text string) bool {
	for _, n := range list.Content {
		if n.Value == text {
			return true
		}
	}
	return false
}

// freeName is base, or base with the first number that makes it a name no rule has.
func freeName(rules *yaml.Node, base string) string {
	name := base
	for n := 2; value(rules, name) != nil; n++ {
		name = fmt.Sprintf("%s-%d", base, n)
	}
	return name
}

// entryLine reads the rendered document back and finds the line of the entry under the rule's allow.
func entryLine(out []byte, rule, entry string) (int, error) {
	doc, err := load(out)
	if err != nil || doc == nil {
		return 0, fmt.Errorf("%w: the rendered document does not read back", ErrInvalid)
	}
	allow := value(value(value(doc.Content[0], "rules"), rule), "allow")
	if allow.Kind == yaml.ScalarNode {
		return allow.Line, nil
	}
	for _, n := range allow.Content {
		if n.Value == entry {
			return n.Line, nil
		}
	}
	return 0, fmt.Errorf("%w: the rendered document lost the entry", ErrInvalid)
}
