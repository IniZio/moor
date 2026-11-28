package yamlfile

import (
	"strconv"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

// RenderSettings prints typed settings, such as ones read back from a store.
func RenderSettings(s domain.Settings) ([]byte, error) { return SettingsDoc{settings: s}.Render() }

func str(s string) *yaml.Node { return &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!str", Value: s} }

func mapping(pairs ...*yaml.Node) *yaml.Node {
	return &yaml.Node{Kind: yaml.MappingNode, Tag: "!!map", Content: pairs}
}

func seq(flow bool, items []string) *yaml.Node {
	n := &yaml.Node{Kind: yaml.SequenceNode, Tag: "!!seq"}
	if flow {
		n.Style = yaml.FlowStyle
	}
	for _, s := range items {
		n.Content = append(n.Content, str(s))
	}
	return n
}

// fresh is the YAML tree of typed settings, in the order of the type.
func fresh(s domain.Settings) *yaml.Node {
	root := mapping()
	add := func(key string, val *yaml.Node) { root.Content = append(root.Content, str(key), val) }
	if len(s.Repositories) > 0 {
		repos := make([]string, len(s.Repositories))
		for i, r := range s.Repositories {
			repos[i] = r.String()
		}
		add("repositories", seq(false, repos))
	}
	if len(s.Credentials) > 0 {
		creds := mapping()
		for _, c := range s.Credentials {
			creds.Content = append(creds.Content, str(c.Name), credentialNode(c))
		}
		add("credentials", creds)
	}
	if len(s.Rules) > 0 {
		rules := mapping()
		for _, r := range s.Rules {
			rules.Content = append(rules.Content, str(r.Name), ruleNode(r))
		}
		add("rules", rules)
	}
	return &yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{root}}
}

func credentialNode(c domain.Credential) *yaml.Node {
	if c.App == nil {
		return mapping(str("value"), str(string(c.Value)))
	}
	app := mapping(str("app_id"), &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!int", Value: strconv.FormatInt(c.App.AppID, 10)},
		str("private_key"), str(string(c.App.PrivateKey)), str("webhook_secret"), str(string(c.App.WebhookSecret)))
	if c.App.APIURL != "" {
		app.Content = append(app.Content, str("api_url"), str(c.App.APIURL))
	}
	return mapping(str("github_app"), app)
}

func ruleNode(r domain.Rule) *yaml.Node {
	hosts := make([]string, len(r.Hosts))
	for i, h := range r.Hosts {
		hosts[i] = h.String()
	}
	n := mapping(str("hosts"), seq(true, hosts))
	add := func(key string, val *yaml.Node) { n.Content = append(n.Content, str(key), val) }
	if r.Credential != "" {
		add("credential", str(r.Credential))
		add("inject", injectNode(r.Inject))
	}
	add("allow", accessNode(r.Allow))
	if len(r.Ask) > 0 {
		add("ask", seq(false, entryTexts(r.Ask)))
	}
	if len(r.BoxEnv) > 0 {
		env := mapping()
		env.Style = yaml.FlowStyle
		for _, v := range r.BoxEnv {
			env.Content = append(env.Content, str(v.Name), str(v.Value))
		}
		add("box_env", env)
	}
	if len(r.GraphQLMutations) > 0 {
		add("graphql_mutations", seq(false, r.GraphQLMutations))
	}
	return n
}

func injectNode(in domain.Inject) *yaml.Node {
	var n *yaml.Node
	switch i := in.(type) {
	case domain.HeaderInject:
		n = mapping(str("header"), str(i.Name), str("form"), str(i.Form))
	case domain.BasicInject:
		n = mapping(str("basic_user"), str(i.User))
	default:
		n = mapping()
	}
	n.Style = yaml.FlowStyle
	return n
}

func accessNode(a domain.Access) *yaml.Node {
	if a.Any {
		return &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!str", Value: domain.AnyText, Style: yaml.DoubleQuotedStyle}
	}
	return seq(false, entryTexts(a.Entries))
}

func entryTexts(list []domain.Entry) []string {
	out := make([]string, len(list))
	for i, e := range list {
		out[i] = e.String()
	}
	return out
}
