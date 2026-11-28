package domain

import (
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"
	"unicode"
)

// ErrBadToken is a proxy call whose token is not the task's, or whose task no longer runs.
var ErrBadToken = errors.New("bad proxy token")

// ProxyKind names one of a task's two proxies: the Box egress proxy or the desk proxy.
type ProxyKind string

const (
	BoxProxy  ProxyKind = "box"
	DeskProxy ProxyKind = "desk"
)

func (k ProxyKind) Valid() bool { return k == BoxProxy || k == DeskProxy }

// DenyReason is the x-deny-reason a proxy answers a refused request with.
type DenyReason string

const (
	DenyHost DenyReason = "host_not_allowed"
	// DenyRequest is a host a rule covers, with a method and path none of its entries allows.
	DenyRequest DenyReason = "request_not_allowed"
	// DenyAsk is a request an ask entry holds until a person allows it.
	DenyAsk DenyReason = "needs_approval"
	// DenyGraphQL is a GraphQL request the judge refused: not one operation, or a mutation off the rule's list.
	DenyGraphQL DenyReason = "graphql_not_allowed"
	// DenyMerge is a request that would merge a pull request, which no rule can allow.
	DenyMerge DenyReason = "merge_not_allowed"
)

var denyReasons = []DenyReason{DenyHost, DenyRequest, DenyAsk, DenyGraphQL, DenyMerge}

// Policy is what one proxy of a task enforces, read from one settings version: the rules, which credentials
// of theirs are set, and the task's repository, which {repo} stands for.
type Policy struct {
	Version int
	Repo    Repo
	Rules   []Rule
	Set     map[string]bool
}

// Gives is true when a rule of the policy adds the credential and the credential is set.
func (p Policy) Gives(credential string) bool {
	return p.Set[credential] && slices.ContainsFunc(p.Rules, func(r Rule) bool { return r.Credential == credential })
}

// PolicyFor is what the proxy of kind enforces under these settings, saved as version, for a task on repo. The
// Box egress proxy takes every rule but the model ones. The desk proxy takes the portal, which portal names,
// and the model rules.
func (s Settings) PolicyFor(kind ProxyKind, version int, portal Rule, repo Repo) Policy {
	p := Policy{Version: version, Repo: repo, Set: map[string]bool{}}
	if kind == DeskProxy {
		p.Rules = append(p.Rules, portal)
	}
	for _, r := range s.Rules {
		if r.Model() != (kind == DeskProxy) {
			continue
		}
		p.Rules = append(p.Rules, r)
		if c, ok := s.Credential(r.Credential); ok && c.Set() {
			p.Set[c.Name] = true
		}
	}
	return p
}

// AnyRequest is a rule that lets every request to host and port through.
func AnyRequest(name string, host HostPattern, port int) Rule {
	return Rule{Name: name, Hosts: []HostPort{{Host: host, Port: port}}, Allow: Access{Any: true}}
}

// Suggest is the entry that would allow method on path for a task on repo: the path with {repo} where the
// repository appears. It is empty when no entry can say it, such as a path holding glob characters.
func Suggest(method Method, path string, repo Repo) string {
	segs := strings.Split(path, "/")
	want := strings.ToLower(string(repo.Path))
	for i := 0; want != "" && i+1 < len(segs); i++ {
		name, git := strings.CutSuffix(segs[i+1], ".git")
		if strings.ToLower(segs[i]+"/"+name) != want {
			continue
		}
		mark := RepoMark
		if git {
			mark += ".git"
		}
		segs = append(append(segs[:i:i], mark), segs[i+2:]...)
		break
	}
	text := string(method) + " " + strings.Join(segs, "/")
	if len(text) > 300 || strings.ContainsAny(path, "*{}") {
		return ""
	}
	if _, err := ParseEntry(text); err != nil {
		return ""
	}
	return text
}

// Decision is what a person did with a blocked request. Undecided is none yet.
type Decision string

const (
	Undecided Decision = ""
	Allowed   Decision = "allowed"
	Dismissed Decision = "dismissed"
)

type BlockedID int64

// TunnelMethod is the method of a refused tunnel, whose requests the proxy never saw.
const TunnelMethod Method = "CONNECT"

// BlockedHit is one request a proxy refused, as the proxy reports it. Path has no query. Rule is the rule that
// covers the host, if any, and Entry what Allow would add to it: an entry, * for a refused tunnel, or nothing
// when no entry can allow it. Detail says more, naming GraphQL fields at most, never a value.
type BlockedHit struct {
	Task   TaskID
	Proxy  ProxyKind
	Host   string
	Port   int
	Method Method
	Path   string
	Rule   string
	Entry  string
	Reason DenyReason
	Detail string
	At     time.Time
}

// Validate refuses a report no proxy could have made, so nothing odd lands on a task.
func (h BlockedHit) Validate() error {
	host, err := ParseHostPattern(h.Host)
	switch {
	case err != nil || strings.HasPrefix(string(host), "*"):
		return fmt.Errorf("%w: blocked host %q", ErrBadSettings, h.Host)
	case !h.Proxy.Valid() || h.Port < 1 || h.Port > 65535:
		return fmt.Errorf("%w: blocked request on proxy %q port %d", ErrBadSettings, h.Proxy, h.Port)
	case !slices.Contains(denyReasons, h.Reason):
		return fmt.Errorf("%w: deny reason %q", ErrBadSettings, h.Reason)
	case !knownMethods[h.Method] && h.Method != TunnelMethod:
		return fmt.Errorf("%w: method %q", ErrBadSettings, h.Method)
	case len(h.Path) > 512 || (h.Path != "" && !strings.HasPrefix(h.Path, "/")) || strings.ContainsFunc(h.Path+h.Detail, unicode.IsControl):
		return fmt.Errorf("%w: blocked path or detail", ErrBadSettings)
	case len(h.Detail) > 300 || (h.Rule != "" && !nameText.MatchString(h.Rule)) || !entryOK(h.Entry):
		return fmt.Errorf("%w: blocked rule, entry or detail", ErrBadSettings)
	}
	return nil
}

func entryOK(e string) bool {
	if e == "" || e == AnyText {
		return true
	}
	_, err := ParseEntry(e)
	return err == nil
}

// BlockedRequest is a refused request recorded on its task, one row per proxy, host, port, method, path and
// detail, with how many times it was refused.
type BlockedRequest struct {
	ID        BlockedID
	Task      TaskID
	Proxy     ProxyKind
	Host      string
	Port      int
	Method    Method
	Path      string
	Rule      string
	Entry     string
	Reason    DenyReason
	Detail    string
	Count     int
	FirstAt   time.Time
	LastAt    time.Time
	Decision  Decision
	DecidedBy string
	DecidedAt time.Time
}

// Request is the row as a person reads it, such as fonts.example.com:443 GET /css.
func (b BlockedRequest) Request() string {
	text := HostPort{Host: HostPattern(b.Host), Port: b.Port}.String() + " " + string(b.Method)
	if b.Path != "" {
		text += " " + b.Path
	}
	return text
}

// Allowable is true when Allow can turn the row into an entry: a Box egress request refused by the rules, with
// an entry to add. A blocked desk request is fixed by a rule a person writes, and a merge never.
func (b BlockedRequest) Allowable() bool {
	return b.Proxy == BoxProxy && b.Entry != "" && (b.Reason == DenyHost || b.Reason == DenyRequest || b.Reason == DenyAsk)
}

// AllowedLine is what Allow did: the line of the settings document that now holds the entry, and the version it
// was saved in.
type AllowedLine struct {
	Line    int
	Version int
}
