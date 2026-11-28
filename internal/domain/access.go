package domain

import (
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"
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
	DenyHost   DenyReason = "host_not_allowed"
	DenyMethod DenyReason = "method_not_allowed"
)

// allMethods is every method an allow rule may name, in the order a rule prints them.
var allMethods = []Method{"GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"}

// modelHosts are the API hosts of model vendors. A connection that names one of them is a model
// connection: only the desk proxy adds it, so its value never passes through the Box egress proxy.
var modelHosts = []HostPattern{"api.anthropic.com", "api.openai.com", "generativelanguage.googleapis.com"}

// Model is true when the connection adds its secret on a model vendor's API host.
func (c Connection) Model() bool {
	return slices.ContainsFunc(c.Hosts, func(h HostPattern) bool { return slices.Contains(modelHosts, h) })
}

// Matches is true when the request host is the pattern's host, or under its suffix.
func (p HostPattern) Matches(host string) bool {
	host = strings.ToLower(host)
	return host != "" && !strings.HasPrefix(host, "*") && p.Covers(HostPattern(host))
}

// Policy is what one proxy of a task enforces, read from one settings version: the allow rules and the
// connections it adds, without their values.
type Policy struct {
	Version     int
	Allow       []AllowRule
	Connections []Connection
}

// Check passes a request when an allow rule names its host, port and method. It fails closed.
func (p Policy) Check(host string, port int, m Method) (DenyReason, bool) {
	reason := DenyHost
	for _, r := range p.Allow {
		if r.Port != port || !r.Host.Matches(host) {
			continue
		}
		if slices.Contains(r.Methods, m) {
			return "", true
		}
		reason = DenyMethod
	}
	return reason, false
}

// Tunnel passes a tunnel to host and port when an allow rule names both, whatever its methods: the
// methods inside a tunnel the proxy does not intercept cannot be seen.
func (p Policy) Tunnel(host string, port int) bool {
	return slices.ContainsFunc(p.Allow, func(r AllowRule) bool { return r.Port == port && r.Host.Matches(host) })
}

// ConnectionFor is the connection that adds its secret on host, the first one that names it.
func (p Policy) ConnectionFor(host string) (Connection, bool) {
	for _, c := range p.Connections {
		if slices.ContainsFunc(c.Hosts, func(h HostPattern) bool { return h.Matches(host) }) {
			return c, true
		}
	}
	return Connection{}, false
}

// PolicyFor is what the proxy of kind enforces under these settings, saved as version. The Box egress
// proxy takes the allow rules and every connection but the model ones. The desk proxy takes the fixed
// list: the portal, which portal names, and the API hosts of the model connections on port 443.
func (s Settings) PolicyFor(kind ProxyKind, version int, portal AllowRule) Policy {
	p := Policy{Version: version}
	for _, c := range s.Connections {
		if c.Model() == (kind == DeskProxy) {
			p.Connections = append(p.Connections, c)
		}
	}
	if kind == BoxProxy {
		p.Allow = s.Allow
		return p
	}
	p.Allow = []AllowRule{portal}
	for _, c := range p.Connections {
		for _, h := range c.Hosts {
			p.Allow = append(p.Allow, AllowRule{Host: h, Port: 443, Methods: allMethods})
		}
	}
	return p
}

// AnyMethod is a rule on host and port that takes every method.
func AnyMethod(host HostPattern, port int) AllowRule {
	return AllowRule{Host: host, Port: port, Methods: allMethods}
}

// Decision is what a person did with a blocked request. Undecided is none yet.
type Decision string

const (
	Undecided Decision = ""
	Allowed   Decision = "allowed"
	Dismissed Decision = "dismissed"
)

type BlockedID int64

// BlockedHit is one request a proxy refused, as the proxy reports it.
type BlockedHit struct {
	Task   TaskID
	Proxy  ProxyKind
	Host   string
	Port   int
	Method Method
	Reason DenyReason
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
	case h.Reason != DenyHost && h.Reason != DenyMethod:
		return fmt.Errorf("%w: deny reason %q", ErrBadSettings, h.Reason)
	case len(h.Method) == 0 || len(h.Method) > 16 || strings.ToUpper(string(h.Method)) != string(h.Method):
		return fmt.Errorf("%w: method %q", ErrBadSettings, h.Method)
	}
	return nil
}

// BlockedRequest is a refused request recorded on its task, one row per proxy, host, port and method,
// with how many times it was refused.
type BlockedRequest struct {
	ID        BlockedID
	Task      TaskID
	Proxy     ProxyKind
	Host      string
	Port      int
	Method    Method
	Reason    DenyReason
	Count     int
	FirstAt   time.Time
	LastAt    time.Time
	Decision  Decision
	DecidedBy string
	DecidedAt time.Time
}

// Request is the row as a person reads it, such as fonts.example.com:443 GET.
func (b BlockedRequest) Request() string { return b.Rule().String() }

// Rule is the allow line that would let the request through.
func (b BlockedRequest) Rule() AllowRule {
	return AllowRule{Host: HostPattern(b.Host), Port: b.Port, Methods: []Method{b.Method}}
}

// Allowable is true when Allow can turn the row into an allow line: a Box egress request with a method
// an allow rule may name. A blocked desk request is fixed by a connection instead.
func (b BlockedRequest) Allowable() bool {
	return b.Proxy == BoxProxy && knownMethods[b.Method]
}

// AllowedLine is what Allow did: the line of the settings document that now holds the rule, and the
// version it was saved in.
type AllowedLine struct {
	Line    int
	Version int
}
