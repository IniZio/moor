package yamlfile

import (
	"regexp"
	"slices"
	"strings"
	"unicode"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

var (
	ruleKeys     = []string{"hosts", "credential", "inject", "allow", "ask", "box_env", "graphql_mutations"}
	headerName   = regexp.MustCompile(`^[A-Za-z0-9-]+$`)
	graphQLField = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
)

func (w *walker) rules(n *yaml.Node, creds []domain.Credential) []domain.Rule {
	var out []domain.Rule
	w.named(n, "rules", func(name string, val *yaml.Node) {
		if r, ok := w.rule(name, val, creds); ok {
			out = append(out, r)
		}
	})
	return out
}

func (w *walker) rule(name string, n *yaml.Node, creds []domain.Credential) (domain.Rule, bool) {
	r := domain.Rule{Name: name}
	o := w.object(n, "rule "+name, ruleKeys)
	if o == nil {
		return r, false
	}
	before := len(w.errs)
	r.Hosts = w.ruleHosts(w.required(o, "hosts"))
	r.Allow = w.access(w.required(o, "allow"))
	if v, ok := o.get("ask"); ok {
		r.Ask = w.entries(v, "ask")
	}
	w.credentialOf(o, &r, creds)
	if v, ok := o.get("box_env"); ok {
		r.BoxEnv = w.boxEnv(v, r.Credential)
	}
	if v, ok := o.get("graphql_mutations"); ok {
		r.GraphQLMutations = w.mutations(v)
	}
	return r, len(w.errs) == before
}

func (w *walker) ruleHosts(n *yaml.Node) []domain.HostPort {
	items := w.list(n, "hosts")
	if n != nil && len(items) == 0 && n.Kind == yaml.SequenceNode {
		w.fail(n, "hosts needs at least one host")
	}
	var out []domain.HostPort
	for _, item := range items {
		text, ok := w.text(item, "a host")
		if !ok {
			continue
		}
		h, err := domain.ParseHostPort(text)
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

// access reads allow: "*" for any request, or a list of entries.
func (w *walker) access(n *yaml.Node) domain.Access {
	switch {
	case n == nil:
		return domain.Access{}
	case n.Kind == yaml.ScalarNode && n.Value == domain.AnyText:
		return domain.Access{Any: true}
	case n.Kind != yaml.SequenceNode || len(n.Content) == 0:
		w.fail(n, `allow is "*" or a list of entries such as "GET /**"`)
		return domain.Access{}
	}
	return domain.Access{Entries: w.entries(n, "allow")}
}

func (w *walker) entries(n *yaml.Node, what string) []domain.Entry {
	var out []domain.Entry
	for _, item := range w.list(n, what) {
		text, ok := w.text(item, "an entry")
		if !ok {
			continue
		}
		e, err := domain.ParseEntry(text)
		switch {
		case err != nil:
			w.fail(item, "%v", err)
		case slices.ContainsFunc(out, func(o domain.Entry) bool { return o.String() == e.String() }):
			w.fail(item, "entry %s is listed twice", e)
		default:
			out = append(out, e)
		}
	}
	return out
}

// credentialOf reads credential and inject, which go together, and checks that the credential is in the document.
func (w *walker) credentialOf(o *object, r *domain.Rule, creds []domain.Credential) {
	cn, hasCred := o.get("credential")
	in, hasInject := o.get("inject")
	if hasCred != hasInject {
		w.fail(o.node, "rule %s has credential and inject together, or neither", r.Name)
	}
	if hasCred {
		name, ok := w.text(cn, "credential")
		if ok && !slices.ContainsFunc(creds, func(c domain.Credential) bool { return c.Name == name }) {
			w.fail(cn, "credential %q is not under credentials", name)
		}
		r.Credential = name
	}
	if hasInject {
		r.Inject = w.inject(in)
	}
}

func (w *walker) inject(n *yaml.Node) domain.Inject {
	o := w.object(n, "inject", []string{"header", "form", "basic_user"})
	if o == nil {
		return nil
	}
	if user, ok := o.get("basic_user"); ok {
		text, _ := w.text(user, "basic_user")
		if len(o.order) != 1 || text == "" || strings.ContainsFunc(text, badUser) {
			w.fail(o.node, "inject is {basic_user: name} alone, a name with no colon")
		}
		return domain.BasicInject{User: text}
	}
	name, _ := w.text(w.required(o, "header"), "header")
	form, _ := w.text(w.required(o, "form"), "form")
	if !headerName.MatchString(name) || !strings.Contains(form, domain.SecretMark) || strings.ContainsFunc(form, unicode.IsControl) {
		w.fail(o.node, "inject is {header: Name, form: \"... {secret} ...\"} or {basic_user: name}")
	}
	return domain.HeaderInject{Name: name, Form: form}
}

func badUser(r rune) bool { return r == ':' || unicode.IsControl(r) }

func (w *walker) boxEnv(n *yaml.Node, credential string) []domain.EnvVar {
	o := w.object(n, "box_env", nil)
	if o == nil {
		return nil
	}
	var out []domain.EnvVar
	for _, name := range o.order {
		if _, err := domain.ParseEnvName(name); err != nil {
			w.failAt(keyLine(o.node, name), "%v", err)
			continue
		}
		val, ok := w.text(o.vals[name], "a box_env value")
		switch {
		case !ok:
		case val == domain.PlaceholderText && credential == "":
			w.fail(o.vals[name], "%s is a placeholder, which needs the rule's credential", name)
		case strings.ContainsFunc(val, unicode.IsControl) || strings.ContainsAny(val, "\"\\$`"):
			w.fail(o.vals[name], "%s has a control character, a quote, a backslash, a $ or a backtick", name)
		default:
			out = append(out, domain.EnvVar{Name: name, Value: val})
		}
	}
	return out
}

func (w *walker) mutations(n *yaml.Node) []string {
	var out []string
	for _, item := range w.list(n, "graphql_mutations") {
		text, ok := w.text(item, "a mutation")
		if ok && (!graphQLField.MatchString(text) || slices.Contains(out, text)) {
			w.fail(item, "mutation %q is not a GraphQL field name, or is listed twice", text)
			continue
		}
		if ok {
			out = append(out, text)
		}
	}
	return out
}
