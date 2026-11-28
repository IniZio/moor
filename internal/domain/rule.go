package domain

import (
	"errors"
	"fmt"
	"regexp"
	"slices"
	"strings"
)

// Rule is one named block of the settings for one service: the hosts it covers, the credential it adds and how,
// the requests it allows, the ones it holds for a person to allow, and the variables it sets in the Box.
type Rule struct {
	Name       string
	Hosts      []HostPort
	Credential string
	Inject     Inject
	Allow      Access
	Ask        []Entry
	BoxEnv     []EnvVar
	// GraphQLMutations are the mutations a GraphQL request to GitHub's API may make under this rule.
	GraphQLMutations []string
}

// Access is what a rule allows: any request (allow: "*") or the requests its entries match.
type Access struct {
	Any     bool
	Entries []Entry
}

// AnyText is the allow value that lets every request through.
const AnyText = "*"

// RepoMark stands for the task's repository, owner/name, in a path or a Box variable.
const RepoMark = "{repo}"

// Entry is one allow or ask line, such as GET|POST /repos/{repo}/pulls. In the path * is any text within one
// segment, ** any number of segments, and {repo} the task's repository.
type Entry struct {
	Methods []Method
	Path    string
}

var (
	pathText = regexp.MustCompile(`^/[A-Za-z0-9._~!$&'()*+,;=:@%/-]*$`)
	envName  = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
	reserved = []string{
		"HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "ALL_PROXY", "SSL_CERT_FILE", "NODE_EXTRA_CA_CERTS", "REQUESTS_CA_BUNDLE", "BOX_DNS", "DOCKER_TLS_CERTDIR", "BOX_ENV", "PATH", "HOME", "DOCKER_HOST",
		"TASK_PROMPT", "MOOR_HARNESS", "BOX_SSH", "HARNESS_BOX_SSH", "MOOR_PORTAL", "MOOR_TASK", "MOOR_TASK_TOKEN", "TMPDIR", "SHELL",
	}
	entryHelp = `an entry is "METHODS /path", methods split by | such as GET|POST, and a path where * is one segment, ** any and {repo} the task's repository`
)

// ParseEntry reads "METHODS /path".
func ParseEntry(s string) (Entry, error) {
	methodText, path, ok := strings.Cut(s, " ")
	if !ok || strings.ContainsAny(path, " \t") {
		return Entry{}, fmt.Errorf("%w: %q: %s", ErrBadSettings, s, entryHelp)
	}
	var e Entry
	for _, m := range strings.Split(methodText, "|") {
		if !knownMethods[Method(m)] || slices.Contains(e.Methods, Method(m)) {
			return Entry{}, fmt.Errorf("%w: %q has method %q, which is unknown, not upper-case or repeated", ErrBadSettings, s, m)
		}
		e.Methods = append(e.Methods, Method(m))
	}
	if err := checkPath(path); err != nil {
		return Entry{}, fmt.Errorf("%w: %q: %w", ErrBadSettings, s, err)
	}
	e.Path = path
	return e, nil
}

// checkPath refuses a path with a query, a glob form other than * and ** whole segments, or a brace other than {repo}.
func checkPath(path string) error {
	bare := strings.ReplaceAll(path, RepoMark, "r")
	if !pathText.MatchString(bare) {
		return errors.New("the path starts with / and holds no query, braces but {repo}, ? or [ ]")
	}
	for _, seg := range strings.Split(bare, "/") {
		if strings.Contains(seg, "**") && seg != "**" {
			return errors.New("** stands alone as a segment")
		}
		if seg == "." || seg == ".." {
			return errors.New("the path has a . or .. segment")
		}
	}
	return nil
}

func (e Entry) String() string {
	methods := make([]string, len(e.Methods))
	for i, m := range e.Methods {
		methods[i] = string(m)
	}
	return strings.Join(methods, "|") + " " + e.Path
}

// Inject says how a rule puts its credential into a request. HeaderInject and BasicInject are the only ones.
type Inject interface{ isInject() }

// HeaderInject sets the header Name to Form with {secret} replaced by the value.
type HeaderInject struct {
	Name string
	Form string
}

// BasicInject sets Basic authorization with User and the value as the password, as git over HTTPS takes it.
type BasicInject struct{ User string }

func (HeaderInject) isInject() {}
func (BasicInject) isInject()  {}

// SecretMark is where the value goes in a header form.
const SecretMark = "{secret}"

// PlaceholderText is the box_env value that stands for the credential. The Box gets a fake token in its place,
// and the proxy adds the real value on the wire.
const PlaceholderText = "placeholder"

// EnvVar is one variable a rule sets in the Box, or for a model rule in the harness session on the desk.
type EnvVar struct {
	Name  string
	Value string
}

// ParseEnvName checks a Box variable's name. The variables the product itself sets in the Box are refused.
func ParseEnvName(s string) (string, error) {
	if !envName.MatchString(s) || slices.Contains(reserved, strings.ToUpper(s)) {
		return "", fmt.Errorf("%w: box_env name %q is not a variable name, or is one the product sets", ErrBadSettings, s)
	}
	return s, nil
}

// Placeholder is the fake token a placeholder variable of the rule holds in the Box. It is no secret.
func (r Rule) Placeholder() string { return "moor-placeholder-" + r.Name }

// BoxValue is the value of v in the Box of a task on repo.
func (r Rule) BoxValue(v EnvVar, repo Repo) string {
	if v.Value == PlaceholderText {
		return r.Placeholder()
	}
	return strings.ReplaceAll(v.Value, RepoMark, string(repo.Path))
}

// modelHosts are the API hosts of model vendors. A rule that names one of them is a model rule: only the desk
// proxy enforces it, so its credential never passes through the Box egress proxy.
var modelHosts = []HostPattern{"api.anthropic.com", "api.openai.com", "generativelanguage.googleapis.com"}

// Model is true when the rule covers a model vendor's API host.
func (r Rule) Model() bool {
	return slices.ContainsFunc(r.Hosts, func(h HostPort) bool { return slices.Contains(modelHosts, h.Host) })
}

// Covers is true when one of the rule's hosts matches host and port.
func (r Rule) Covers(host string, port int) bool {
	return slices.ContainsFunc(r.Hosts, func(h HostPort) bool { return h.Matches(host, port) })
}

func (r Rule) validate(creds map[string]bool) error {
	switch {
	case r.Name == "" || len(r.Hosts) == 0:
		return errors.New("a rule needs a name and hosts")
	case (r.Credential == "") != (r.Inject == nil):
		return errors.New("credential and inject go together")
	case r.Credential != "" && !creds[r.Credential]:
		return errors.New("the credential is not in the document")
	case !r.Allow.Any && len(r.Allow.Entries) == 0:
		return errors.New("allow is * or a list of entries")
	}
	for _, v := range r.BoxEnv {
		if v.Value == PlaceholderText && r.Credential == "" {
			return errors.New("a placeholder needs a credential")
		}
	}
	return nil
}
