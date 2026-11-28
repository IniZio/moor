package yamlfile

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"regexp"
	"slices"
	"strconv"
	"strings"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

// ErrInvalid is what every error of this package matches with errors.Is.
var ErrInvalid = errors.New("invalid file")

const maxBytes = 1 << 20

var syntaxLine = regexp.MustCompile(`^line (\d+)`)

// ParseError is one problem, with its line counted from 1, or 0 when it has none.
type ParseError struct {
	Line int
	Msg  string
}

// ParseErrors lists every problem found, in the order of the file.
type ParseErrors []ParseError

func (e ParseErrors) Error() string {
	lines := make([]string, len(e))
	for i, p := range e {
		lines[i] = "line " + strconv.Itoa(p.Line) + ": " + p.Msg
		if p.Line == 0 {
			lines[i] = p.Msg
		}
	}
	return ErrInvalid.Error() + ": " + strings.Join(lines, "; ")
}

func (e ParseErrors) Is(target error) bool { return target == ErrInvalid }

// load reads exactly one YAML document, and nil for no document at all. Aliases are refused.
func load(data []byte) (*yaml.Node, error) {
	if len(data) > maxBytes {
		return nil, ParseErrors{{Msg: "the file is larger than 1 MiB"}}
	}
	dec := yaml.NewDecoder(bytes.NewReader(data))
	var doc yaml.Node
	if err := dec.Decode(&doc); err != nil {
		if errors.Is(err, io.EOF) {
			return nil, nil
		}
		return nil, syntaxError(err)
	}
	var next yaml.Node
	if err := dec.Decode(&next); !errors.Is(err, io.EOF) {
		if err != nil {
			return nil, syntaxError(err)
		}
		return nil, ParseErrors{{Line: next.Line, Msg: "a second YAML document starts here, and only one is read"}}
	}
	if len(doc.Content) == 0 {
		return nil, nil
	}
	if alias := findAlias(&doc); alias != nil {
		return nil, ParseErrors{{Line: alias.Line, Msg: "aliases are not supported"}}
	}
	return &doc, nil
}

func syntaxError(err error) error {
	msg := strings.TrimPrefix(err.Error(), "yaml: ")
	line := 0
	if m := syntaxLine.FindStringSubmatch(msg); m != nil {
		line, _ = strconv.Atoi(m[1])
		msg = strings.TrimPrefix(msg, m[0]+": ")
	}
	return ParseErrors{{Line: line, Msg: msg}}
}

func findAlias(n *yaml.Node) *yaml.Node {
	if n.Kind == yaml.AliasNode {
		return n
	}
	for _, c := range n.Content {
		if a := findAlias(c); a != nil {
			return a
		}
	}
	return nil
}

type walker struct{ errs ParseErrors }

func (w *walker) failAt(line int, format string, args ...any) {
	w.errs = append(w.errs, ParseError{Line: line, Msg: fmt.Sprintf(format, args...)})
}

func (w *walker) fail(n *yaml.Node, format string, args ...any) { w.failAt(n.Line, format, args...) }

// object is a mapping that has passed the key check. order keeps the keys as the file has them.
type object struct {
	node  *yaml.Node
	vals  map[string]*yaml.Node
	order []string
}

func (o *object) get(key string) (*yaml.Node, bool) {
	n, ok := o.vals[key]
	return n, ok
}

// object checks that n is a mapping of text keys, each once. A nil allowed list means any key.
func (w *walker) object(n *yaml.Node, what string, allowed []string) *object {
	if n == nil {
		return nil
	}
	if n.Kind != yaml.MappingNode {
		w.fail(n, "%s must be a mapping", what)
		return nil
	}
	o := &object{node: n, vals: map[string]*yaml.Node{}}
	for i := 0; i+1 < len(n.Content); i += 2 {
		key, val := n.Content[i], n.Content[i+1]
		switch {
		case key.Kind != yaml.ScalarNode:
			w.fail(key, "%s has a key that is not text", what)
		case allowed != nil && !slices.Contains(allowed, key.Value):
			w.fail(key, "unknown key %q in %s, which has %s", key.Value, what, strings.Join(allowed, ", "))
		case o.vals[key.Value] != nil:
			w.fail(key, "key %q is repeated in %s", key.Value, what)
		default:
			o.vals[key.Value] = val
			o.order = append(o.order, key.Value)
		}
	}
	return o
}

// required returns the value of key, or nil after reporting that it is missing.
func (w *walker) required(o *object, key string) *yaml.Node {
	if o == nil {
		return nil
	}
	n, ok := o.get(key)
	if !ok {
		w.fail(o.node, "missing required key %q", key)
	}
	return n
}

func (w *walker) text(n *yaml.Node, what string) (string, bool) {
	if n == nil {
		return "", false
	}
	if n.Kind != yaml.ScalarNode || n.ShortTag() != "!!str" {
		w.fail(n, "%s must be text", what)
		return "", false
	}
	return n.Value, true
}

func (w *walker) flag(n *yaml.Node, what string) bool {
	if n.Kind != yaml.ScalarNode || n.ShortTag() != "!!bool" {
		w.fail(n, "%s must be true or false", what)
		return false
	}
	return n.Value == "true"
}

func (w *walker) list(n *yaml.Node, what string) []*yaml.Node {
	if n == nil {
		return nil
	}
	if n.Kind != yaml.SequenceNode {
		w.fail(n, "%s must be a list", what)
		return nil
	}
	return n.Content
}

func isNull(n *yaml.Node) bool { return n.Kind == yaml.ScalarNode && n.ShortTag() == "!!null" }

// repositories reads a list of repositories, each once, in the host/path or owner/name form.
func (w *walker) repositories(n *yaml.Node) []domain.Repo {
	seen := map[domain.Repo]bool{}
	var out []domain.Repo
	for _, item := range w.list(n, "repositories") {
		text, ok := w.text(item, "a repository")
		if !ok {
			continue
		}
		r, err := domain.ParseRepo(text)
		switch {
		case err != nil:
			w.fail(item, "%v", err)
		case seen[r]:
			w.fail(item, "repository %s is listed twice", r)
		default:
			seen[r] = true
			out = append(out, r)
		}
	}
	return out
}
