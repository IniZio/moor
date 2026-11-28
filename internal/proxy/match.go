package proxy

import (
	"slices"
	"strings"

	"github.com/bmatcuk/doublestar/v4"

	"github.com/IniZio/moor/internal/domain"
)

// githubAPI is the host whose GraphQL requests the judge reads and whose tunnels are always intercepted, so that
// no rule can let a merge through unseen.
const (
	githubAPI   = "api.github.com"
	graphQLPath = "/graphql"
)

type route struct {
	method  domain.Method
	pattern string
}

// mergeRoutes are the REST routes that merge, refused on every host whatever the rules say.
var mergeRoutes = []route{
	{"PUT", "**/repos/*/*/pulls/*/merge"},
	{"POST", "**/repos/*/*/merges"},
	{"POST", "**/repos/*/*/merge-upstream"},
}

// verdict is what the policy says of one request: the rule that passes it, or why it is refused, with the rule
// that covers its host and the entry Allow would add.
type verdict struct {
	pass   *domain.Rule
	reason domain.DenyReason
	detail string
	rule   string
	entry  string
}

// covering lists the rules that name host and port.
func covering(pol *domain.Policy, host string, port int) []domain.Rule {
	var out []domain.Rule
	for _, r := range pol.Rules {
		if r.Covers(host, port) {
			out = append(out, r)
		}
	}
	return out
}

// decide judges one request seen in full. A merge is refused first, an ask entry holds the request next, and
// then the first rule that allows it passes it. It fails closed.
func decide(pol *domain.Policy, host string, port int, method domain.Method, path string) verdict {
	rules := covering(pol, host, port)
	if len(rules) == 0 {
		return verdict{reason: domain.DenyHost, entry: domain.Suggest(method, path, pol.Repo)}
	}
	refused := verdict{reason: domain.DenyRequest, rule: rules[0].Name, entry: domain.Suggest(method, path, pol.Repo)}
	if !clean(path) {
		refused.entry = ""
		return refused
	}
	if mergesREST(method, path) {
		return verdict{reason: domain.DenyMerge, detail: mergeMessage, rule: rules[0].Name}
	}
	repo := string(pol.Repo.Path)
	for _, r := range rules {
		if i := slices.IndexFunc(r.Ask, func(e domain.Entry) bool { return matches(e, repo, method, path) }); i >= 0 {
			return verdict{reason: domain.DenyAsk, rule: r.Name, entry: r.Ask[i].String()}
		}
	}
	for _, r := range rules {
		if r.Allow.Any || slices.ContainsFunc(r.Allow.Entries, func(e domain.Entry) bool { return matches(e, repo, method, path) }) {
			return verdict{pass: &r}
		}
	}
	return refused
}

// intercepts is true when a tunnel to host must be opened to judge its requests: a rule that covers it lists
// paths, asks, or adds a credential, or the host is GitHub's API. Any other allowed host is tunnelled untouched.
func intercepts(rules []domain.Rule, host string) bool {
	return host == githubAPI || slices.ContainsFunc(rules, func(r domain.Rule) bool {
		return !r.Allow.Any || len(r.Ask) > 0 || r.Credential != ""
	})
}

// matches is true when the entry names the method and its path, with {repo} as the task's repository, matches.
// Paths compare without case, as GitHub's owner and repository names do.
func matches(e domain.Entry, repo string, method domain.Method, path string) bool {
	if !slices.Contains(e.Methods, method) || (repo == "" && strings.Contains(e.Path, domain.RepoMark)) {
		return false
	}
	pattern := strings.ToLower(strings.ReplaceAll(e.Path, domain.RepoMark, repo))
	ok, err := doublestar.Match(pattern, strings.ToLower(path))
	return err == nil && ok
}

func mergesREST(method domain.Method, path string) bool {
	path = strings.ToLower(strings.TrimSuffix(path, "/"))
	return slices.ContainsFunc(mergeRoutes, func(m route) bool {
		ok, err := doublestar.Match(m.pattern, path)
		return m.method == method && err == nil && ok
	})
}

// clean is true for a path with no empty, . or .. segment past its first slash and no escaped slash, dot or
// backslash, any of which an upstream might fold into another path than the one judged.
func clean(path string) bool {
	lower := strings.ToLower(path)
	if !strings.HasPrefix(path, "/") || strings.Contains(lower, "%2f") || strings.Contains(lower, "%2e") || strings.Contains(lower, "%5c") {
		return false
	}
	segs := strings.Split(path[1:], "/")
	for i, s := range segs {
		if s == "." || s == ".." || (s == "" && i != len(segs)-1) {
			return false
		}
	}
	return true
}
