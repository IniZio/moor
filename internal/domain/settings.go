package domain

import (
	"errors"
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

var ErrBadSettings = errors.New("bad settings")

type (
	// HostPattern is a lower-case host, or *.suffix for any host under the suffix at any depth.
	HostPattern string
	Method      string
	// SecretState is what the settings say about a connection's secret, never the secret itself.
	SecretState string
)

const (
	SecretSet    SecretState = "set"
	SecretNotSet SecretState = "not set"
)

var knownMethods = map[Method]bool{
	"GET": true, "HEAD": true, "POST": true, "PUT": true, "PATCH": true, "DELETE": true, "OPTIONS": true,
}

var hostLabel = regexp.MustCompile(`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`)

// AllowRule lets the proxy pass a request to Host on Port with one of Methods.
type AllowRule struct {
	Host    HostPattern
	Port    int
	Methods []Method
}

// Connection adds a secret to requests to Hosts by Add. Name is unique in a document.
type Connection struct {
	Name   string
	Hosts  []HostPattern
	Add    AddRule
	Secret SecretState
}

// NewSecret is a secret value a saved document carried in its value field, lifted out of the document.
type NewSecret struct {
	Connection string
	Value      string
}

// Settings is the org settings document: the repositories, the allow rules and the connections.
type Settings struct {
	Repositories []Repo
	Allow        []AllowRule
	Connections  []Connection
}

// UncoveredHost is a connection host that no allow rule covers.
type UncoveredHost struct {
	Connection string
	Host       HostPattern
}

func (u UncoveredHost) String() string {
	return fmt.Sprintf("connection %q adds a secret to %s, which no allow rule covers", u.Connection, u.Host)
}

// ParseHostPattern lower-cases and checks a host or a *.suffix.
func ParseHostPattern(s string) (HostPattern, error) {
	host := strings.ToLower(s)
	body := strings.TrimPrefix(host, "*.")
	if len(host) > 253 || body == "" {
		return "", fmt.Errorf("%w: host %q", ErrBadSettings, s)
	}
	for _, label := range strings.Split(body, ".") {
		if len(label) > 63 || !hostLabel.MatchString(label) {
			return "", fmt.Errorf("%w: host %q is not a host name or *.suffix", ErrBadSettings, s)
		}
	}
	return HostPattern(host), nil
}

func (p HostPattern) suffix() (string, bool) {
	rest, wild := strings.CutPrefix(string(p), "*")
	return rest, wild
}

// Covers is true when every host that other matches is also matched by p.
func (p HostPattern) Covers(other HostPattern) bool {
	suffix, wild := p.suffix()
	switch {
	case !wild:
		return p == other
	case strings.HasPrefix(string(other), "*"):
		inner, _ := other.suffix()
		return strings.HasSuffix(inner, suffix)
	}
	return strings.HasSuffix(string(other), suffix)
}

// ParseAllowRule reads "host:port METHOD...", with upper-case methods and no method twice.
func ParseAllowRule(s string) (AllowRule, error) {
	fields := strings.Fields(s)
	if len(fields) < 2 {
		return AllowRule{}, fmt.Errorf("%w: allow rule %q needs host:port and at least one method", ErrBadSettings, s)
	}
	hostText, portText, ok := cutLast(fields[0], ":")
	host, err := ParseHostPattern(hostText)
	port, portOK := digits(portText)
	if !ok || err != nil || !portOK || port < 1 || port > 65535 {
		return AllowRule{}, fmt.Errorf("%w: allow rule %q needs host:port, a host or *.suffix and a port of 1 to 65535", ErrBadSettings, s)
	}
	methods := make([]Method, 0, len(fields)-1)
	for _, f := range fields[1:] {
		m := Method(f)
		if !knownMethods[m] || slices.Contains(methods, m) {
			return AllowRule{}, fmt.Errorf("%w: allow rule %q has method %q, which is unknown, not upper-case or repeated", ErrBadSettings, s, f)
		}
		methods = append(methods, m)
	}
	return AllowRule{Host: host, Port: port, Methods: methods}, nil
}

func digits(s string) (int, bool) {
	for _, r := range s {
		if r < '0' || r > '9' {
			return 0, false
		}
	}
	n, err := strconv.Atoi(s)
	return n, err == nil
}

func cutLast(s, sep string) (string, string, bool) {
	i := strings.LastIndex(s, sep)
	if i < 0 {
		return s, "", false
	}
	return s[:i], s[i+len(sep):], true
}

func (r AllowRule) String() string {
	methods := make([]string, len(r.Methods))
	for i, m := range r.Methods {
		methods[i] = string(m)
	}
	return string(r.Host) + ":" + strconv.Itoa(r.Port) + " " + strings.Join(methods, " ")
}

// Validate refuses a document a parser could not have made: unnamed or repeated connections, a connection
// without hosts or an add rule, an unknown secret state, and incomplete or repeated repositories.
func (s Settings) Validate() error {
	seen := make(map[Repo]bool, len(s.Repositories))
	for _, r := range s.Repositories {
		if !r.Complete() || seen[r] {
			return fmt.Errorf("%w: incomplete or repeated repository %q", ErrBadSettings, r)
		}
		seen[r] = true
	}
	names := make(map[string]bool, len(s.Connections))
	for _, c := range s.Connections {
		if c.Name == "" || names[c.Name] || len(c.Hosts) == 0 || c.Add == nil || (c.Secret != SecretSet && c.Secret != SecretNotSet) {
			return fmt.Errorf("%w: connection %q is unnamed, repeated, without hosts or an add rule, or has an unknown secret state", ErrBadSettings, c.Name)
		}
		names[c.Name] = true
	}
	return nil
}

// UncoveredHosts lists the connection hosts that no allow rule covers, which a person should read as a
// warning: the proxy would add the secret to requests it then refuses. Ports and methods do not matter here.
func (s Settings) UncoveredHosts() []UncoveredHost {
	var out []UncoveredHost
	for _, c := range s.Connections {
		for _, h := range c.Hosts {
			covered := slices.ContainsFunc(s.Allow, func(r AllowRule) bool { return r.Host.Covers(h) })
			if !covered {
				out = append(out, UncoveredHost{Connection: c.Name, Host: h})
			}
		}
	}
	return out
}
