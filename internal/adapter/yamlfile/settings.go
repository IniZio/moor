package yamlfile

import (
	"bytes"
	"fmt"
	"strconv"

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

// ParseSettings reads a settings document that is being saved. A secret of set keeps the stored secret, not set
// means none, and anything else is a new secret: it comes back in the list, and the document and its errors
// never hold it.
func ParseSettings(data []byte) (SettingsDoc, []domain.NewSecret, error) {
	doc, err := load(data)
	if err != nil {
		return SettingsDoc{}, nil, err
	}
	if doc == nil || isNull(doc.Content[0]) {
		return SettingsDoc{}, nil, nil
	}
	w := &walker{}
	s, found := w.settings(doc.Content[0])
	if len(w.errs) > 0 {
		return SettingsDoc{}, nil, w.errs
	}
	if err := s.Validate(); err != nil {
		return SettingsDoc{}, nil, fmt.Errorf("%w: %w", ErrInvalid, err)
	}
	secrets := make([]domain.NewSecret, len(found))
	for i, l := range found {
		secrets[i] = l.secret
		scrub(l.node)
	}
	return SettingsDoc{settings: s, src: doc}, secrets, nil
}

type lifted struct {
	secret domain.NewSecret
	node   *yaml.Node
}

// oldKeys are the keys of the settings before credentials and rules. A document holding one is refused by name.
var oldKeys = map[string]string{
	"allow":       "allow is now per rule: write a block under rules with hosts and allow",
	"connections": "connections are now credentials and rules: put the secret under credentials and its hosts and inject under rules",
}

func (w *walker) settings(root *yaml.Node) (domain.Settings, []lifted) {
	var s domain.Settings
	o := w.object(root, "the file", []string{"repositories", "credentials", "rules", "allow", "connections"})
	if o == nil {
		return s, nil
	}
	for _, key := range o.order {
		if msg, old := oldKeys[key]; old {
			w.failAt(keyLine(o.node, key), "%s", msg)
		}
	}
	if n, ok := o.get("repositories"); ok {
		s.Repositories = w.repositories(n)
	}
	var found []lifted
	if n, ok := o.get("credentials"); ok {
		s.Credentials, found = w.credentials(n)
	}
	if n, ok := o.get("rules"); ok {
		s.Rules = w.rules(n, s.Credentials)
	}
	return s, found
}

// keyLine is the line of key in a mapping.
func keyLine(m *yaml.Node, key string) int {
	for i := 0; i+1 < len(m.Content); i += 2 {
		if m.Content[i].Value == key {
			return m.Content[i].Line
		}
	}
	return m.Line
}

func scrub(n *yaml.Node) {
	n.Value, n.Style, n.Tag, n.LineComment = string(domain.SecretSet), 0, "!!str", ""
}

// named walks a mapping of names to blocks, checking each name.
func (w *walker) named(n *yaml.Node, what string, each func(name string, val *yaml.Node)) {
	o := w.object(n, what, nil)
	if o == nil {
		return
	}
	for _, name := range o.order {
		if _, err := domain.ParseName(name); err != nil {
			w.failAt(keyLine(o.node, name), "%v", err)
			continue
		}
		each(name, o.vals[name])
	}
}

func (w *walker) credentials(n *yaml.Node) ([]domain.Credential, []lifted) {
	var out []domain.Credential
	var found []lifted
	w.named(n, "credentials", func(name string, val *yaml.Node) {
		c, l, ok := w.credential(name, val)
		if ok {
			out = append(out, c)
			found = append(found, l...)
		}
	})
	return out, found
}

func (w *walker) credential(name string, n *yaml.Node) (domain.Credential, []lifted, bool) {
	c := domain.Credential{Name: name}
	o := w.object(n, "credential "+name, []string{"value", "github_app"})
	if o == nil {
		return c, nil, false
	}
	before := len(w.errs)
	val, hasValue := o.get("value")
	app, hasApp := o.get("github_app")
	var found []lifted
	switch {
	case hasValue == hasApp:
		w.fail(o.node, "credential %s has value or github_app, one of them", name)
	case hasValue:
		var l *lifted
		c.Value, l = w.secret(val, domain.SecretKey(name, domain.FieldValue))
		found = appendLifted(found, l)
	default:
		c.App, found = w.githubApp(name, app)
	}
	return c, found, len(w.errs) == before
}

func appendLifted(list []lifted, l *lifted) []lifted {
	if l == nil {
		return list
	}
	return append(list, *l)
}

func (w *walker) githubApp(name string, n *yaml.Node) (*domain.GitHubApp, []lifted) {
	o := w.object(n, "github_app", []string{"app_id", "private_key", "webhook_secret", "api_url"})
	if o == nil {
		return nil, nil
	}
	app := &domain.GitHubApp{WebhookSecret: domain.SecretNotSet}
	if id := w.required(o, "app_id"); id != nil {
		n, err := strconv.ParseInt(id.Value, 10, 64)
		if id.ShortTag() != "!!int" || err != nil || n <= 0 {
			w.fail(id, "app_id must be the number of the GitHub App")
		}
		app.AppID = n
	}
	var found []lifted
	var l *lifted
	app.PrivateKey, l = w.secret(w.required(o, "private_key"), domain.SecretKey(name, domain.FieldPrivateKey))
	found = appendLifted(found, l)
	if ws, ok := o.get("webhook_secret"); ok {
		app.WebhookSecret, l = w.secret(ws, domain.SecretKey(name, domain.FieldWebhookSecret))
		found = appendLifted(found, l)
	}
	if u, ok := o.get("api_url"); ok {
		app.APIURL, _ = w.text(u, "api_url")
	}
	return app, found
}

// secret never puts the value text into a message.
func (w *walker) secret(n *yaml.Node, key string) (domain.SecretState, *lifted) {
	if n == nil {
		return "", nil
	}
	if n.Kind != yaml.ScalarNode || isNull(n) || n.Value == "" {
		w.fail(n, "a secret must be set, not set, or the secret itself")
		return "", nil
	}
	switch domain.SecretState(n.Value) {
	case domain.SecretSet, domain.SecretNotSet:
		return domain.SecretState(n.Value), nil
	}
	return domain.SecretSet, &lifted{secret: domain.NewSecret{Key: key, Value: n.Value}, node: n}
}

// Render prints the document with every secret as set or not set. A document that came from ParseSettings keeps
// its comments, key order and scalar styles, and any other prints in the order of the type.
func (d SettingsDoc) Render() ([]byte, error) {
	src := d.src
	if src == nil {
		src = fresh(d.settings)
	}
	var buf bytes.Buffer
	enc := yaml.NewEncoder(&buf)
	enc.SetIndent(2)
	err := enc.Encode(src)
	if err == nil {
		err = enc.Close()
	}
	if err != nil {
		return nil, fmt.Errorf("render settings: %w", err)
	}
	return buf.Bytes(), nil
}
