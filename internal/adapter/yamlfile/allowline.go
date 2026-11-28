package yamlfile

import (
	"fmt"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

// AppendAllow adds rule as the last line under allow, keeping the rest of the document as it is. A line of
// the same text already there is kept instead. It returns the document and the line number holding the rule.
func AppendAllow(text []byte, rule string) ([]byte, int, error) {
	if _, err := domain.ParseAllowRule(rule); err != nil {
		return nil, 0, fmt.Errorf("%w: %w", ErrInvalid, err)
	}
	doc, secrets, err := ParseSettings(text)
	if err != nil {
		return nil, 0, err
	}
	if len(secrets) > 0 {
		return nil, 0, fmt.Errorf("%w: the document holds a typed value", ErrInvalid)
	}
	if doc.src == nil {
		doc.src = &yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{{Kind: yaml.MappingNode, Tag: "!!map"}}}
	}
	list := allowList(doc.src.Content[0])
	if !hasLine(list, rule) {
		list.Content = append(list.Content, &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!str", Value: rule})
	}
	out, err := doc.Render()
	if err != nil {
		return nil, 0, err
	}
	line, err := lineOf(out, rule)
	return out, line, err
}

// allowList is the sequence under the allow key, made a block sequence so an appended line gets a line of
// its own, and created when the key is missing. ParseSettings has already refused an allow that is no list.
func allowList(root *yaml.Node) *yaml.Node {
	for i := 0; i+1 < len(root.Content); i += 2 {
		if root.Content[i].Value == "allow" {
			v := root.Content[i+1]
			v.Style = 0
			return v
		}
	}
	v := &yaml.Node{Kind: yaml.SequenceNode, Tag: "!!seq"}
	root.Content = append(root.Content, &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!str", Value: "allow"}, v)
	return v
}

func hasLine(list *yaml.Node, rule string) bool {
	for _, n := range list.Content {
		if n.Value == rule {
			return true
		}
	}
	return false
}

// lineOf reads the rendered document back and finds the line of the rule under allow.
func lineOf(out []byte, rule string) (int, error) {
	doc, err := load(out)
	if err != nil || doc == nil {
		return 0, fmt.Errorf("%w: the rendered document does not read back", ErrInvalid)
	}
	list := allowList(doc.Content[0])
	for _, n := range list.Content {
		if n.Value == rule {
			return n.Line, nil
		}
	}
	return 0, fmt.Errorf("%w: the rendered document lost the rule", ErrInvalid)
}
