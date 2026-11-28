package yamlfile

import (
	"bytes"
	"fmt"
	"slices"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

// SettingsDoc is a parsed settings document. It keeps the YAML tree of the text it came from, with every
// secret already replaced by its state, so that Render keeps the comments, key order and quoting of the text.
type SettingsDoc struct {
	settings domain.Settings
	src      *yaml.Node
}

func (d SettingsDoc) Settings() domain.Settings { return d.settings }

// ParseSettings reads a settings document that is being saved. A value of set keeps the stored secret, a
// value of not set means no secret, and anything else is a new secret: it comes back in the list, and the
// document and its errors never hold it.
func ParseSettings(data []byte) (SettingsDoc, []domain.NewSecret, error) {
	doc, err := load(data)
	if err != nil {
		return SettingsDoc{}, nil, err
	}
	if doc == nil {
		return SettingsDoc{}, nil, nil
	}
	if isNull(doc.Content[0]) {
		return SettingsDoc{}, nil, nil
	}
	w := &walker{}
	s, secrets := w.settings(doc.Content[0])
	if len(w.errs) > 0 {
		return SettingsDoc{}, nil, w.errs
	}
	if err := s.Validate(); err != nil {
		return SettingsDoc{}, nil, fmt.Errorf("%w: %w", ErrInvalid, err)
	}
	return SettingsDoc{settings: s, src: doc}, secrets, nil
}

type lifted struct {
	secret domain.NewSecret
	node   *yaml.Node
}

func (w *walker) settings(root *yaml.Node) (domain.Settings, []domain.NewSecret) {
	var s domain.Settings
	o := w.object(root, "the file", []string{"repositories", "allow", "connections"})
	if o == nil {
		return s, nil
	}
	if n, ok := o.get("repositories"); ok {
		s.Repositories = w.repositories(n)
	}
	if n, ok := o.get("allow"); ok {
		s.Allow = w.allow(n)
	}
	var found []lifted
	if n, ok := o.get("connections"); ok {
		s.Connections, found = w.connections(n)
	}
	var secrets []domain.NewSecret
	for _, l := range found {
		secrets = append(secrets, l.secret)
		scrub(l.node)
	}
	return s, secrets
}

func scrub(n *yaml.Node) {
	n.Value, n.Style, n.Tag, n.LineComment = string(domain.SecretSet), 0, "!!str", ""
}

func (w *walker) allow(n *yaml.Node) []domain.AllowRule {
	var out []domain.AllowRule
	for _, item := range w.list(n, "allow") {
		text, ok := w.text(item, "an allow rule")
		if !ok {
			continue
		}
		rule, err := domain.ParseAllowRule(text)
		if err != nil {
			w.fail(item, "%v", err)
			continue
		}
		out = append(out, rule)
	}
	return out
}

func (w *walker) connections(n *yaml.Node) ([]domain.Connection, []lifted) {
	var out []domain.Connection
	var found []lifted
	names := map[string]bool{}
	for _, item := range w.list(n, "connections") {
		c, l, ok := w.connection(item, names)
		if !ok {
			continue
		}
		out = append(out, c)
		if l != nil {
			found = append(found, *l)
		}
	}
	return out, found
}

func (w *walker) connection(n *yaml.Node, names map[string]bool) (domain.Connection, *lifted, bool) {
	var c domain.Connection
	o := w.object(n, "a connection", []string{"name", "hosts", "add", "value"})
	if o == nil {
		return c, nil, false
	}
	before := len(w.errs)
	name, named := w.text(w.required(o, "name"), "name")
	switch {
	case named && name == "":
		w.fail(o.vals["name"], "name is empty")
	case named && names[name]:
		w.fail(o.vals["name"], "connection name %q is used twice", name)
	}
	c.Name, names[name] = name, true
	c.Hosts = w.hosts(w.required(o, "hosts"))
	c.Add = w.addRule(w.required(o, "add"))
	var l *lifted
	c.Secret, l = w.secret(w.required(o, "value"), c.Name)
	return c, l, len(w.errs) == before
}

func (w *walker) hosts(n *yaml.Node) []domain.HostPattern {
	items := w.list(n, "hosts")
	if n != nil && len(items) == 0 && n.Kind == yaml.SequenceNode {
		w.fail(n, "hosts needs at least one host")
	}
	var out []domain.HostPattern
	for _, item := range items {
		text, ok := w.text(item, "a host")
		if !ok {
			continue
		}
		h, err := domain.ParseHostPattern(text)
		switch {
		case err != nil:
			w.fail(item, "%v", err)
		case slices.Contains(out, h):
			w.fail(item, "host %s is listed twice", h)
		default:
			out = append(out, h)
		}
	}
	return out
}

func (w *walker) addRule(n *yaml.Node) domain.AddRule {
	text, ok := w.text(n, "add")
	if !ok {
		return nil
	}
	rule, err := domain.ParseAddRule(text)
	if err != nil {
		w.fail(n, "%v", err)
	}
	return rule
}

// secret never puts the value text into a message.
func (w *walker) secret(n *yaml.Node, name string) (domain.SecretState, *lifted) {
	if n == nil {
		return "", nil
	}
	if n.Kind != yaml.ScalarNode || isNull(n) || n.Value == "" {
		w.fail(n, "value must be set, not set, or the secret itself")
		return "", nil
	}
	switch domain.SecretState(n.Value) {
	case domain.SecretSet, domain.SecretNotSet:
		return domain.SecretState(n.Value), nil
	}
	return domain.SecretSet, &lifted{secret: domain.NewSecret{Connection: name, Value: n.Value}, node: n}
}

// Render prints the document with every value as set or not set. A document that came from ParseSettings
// keeps its comments, key order and scalar styles, and any other prints in the order of the type.
func (d SettingsDoc) Render() ([]byte, error) {
	var buf bytes.Buffer
	enc := yaml.NewEncoder(&buf)
	enc.SetIndent(2)
	var err error
	if d.src != nil {
		err = enc.Encode(d.src)
	} else {
		err = enc.Encode(fresh(d.settings))
	}
	if err == nil {
		err = enc.Close()
	}
	if err != nil {
		return nil, fmt.Errorf("render settings: %w", err)
	}
	return buf.Bytes(), nil
}

// RenderSettings prints typed settings, such as ones read back from a store.
func RenderSettings(s domain.Settings) ([]byte, error) { return SettingsDoc{settings: s}.Render() }

type outConnection struct {
	Name  string   `yaml:"name"`
	Hosts []string `yaml:"hosts,flow"`
	Add   string   `yaml:"add"`
	Value string   `yaml:"value"`
}

type outSettings struct {
	Repositories []string        `yaml:"repositories,omitempty"`
	Allow        []string        `yaml:"allow,omitempty"`
	Connections  []outConnection `yaml:"connections,omitempty"`
}

func fresh(s domain.Settings) outSettings {
	var out outSettings
	for _, r := range s.Repositories {
		out.Repositories = append(out.Repositories, r.String())
	}
	for _, a := range s.Allow {
		out.Allow = append(out.Allow, a.String())
	}
	for _, c := range s.Connections {
		oc := outConnection{Name: c.Name, Add: c.Add.String(), Value: string(c.Secret)}
		for _, h := range c.Hosts {
			oc.Hosts = append(oc.Hosts, string(h))
		}
		out.Connections = append(out.Connections, oc)
	}
	return out
}
