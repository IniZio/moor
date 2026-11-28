package domain

import (
	"errors"
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

var ErrBadSettings = errors.New("bad settings")

type (
	// HostPattern is a lower-case host, or *.suffix for any host under the suffix at any depth.
	HostPattern string
	Method      string
	// SecretState is what the settings say about a secret, never the secret itself.
	SecretState string
)

const (
	SecretSet    SecretState = "set"
	SecretNotSet SecretState = "not set"
)

var knownMethods = map[Method]bool{
	"GET": true, "HEAD": true, "POST": true, "PUT": true, "PATCH": true, "DELETE": true, "OPTIONS": true,
}

var (
	hostLabel = regexp.MustCompile(`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`)
	// nameText is the form of a credential or rule name: a map key a person types, safe in a secret's key.
	nameText = regexp.MustCompile(`^[a-z0-9]([a-z0-9._-]*[a-z0-9])?$`)
)

// Settings is the org settings document: the repositories, the credentials and the rules.
type Settings struct {
	Repositories []Repo
	Credentials  []Credential
	Rules        []Rule
}

// NewSecret is a secret value a saved document carried in place of set, lifted out of the document. Key names
// the credential and its field, as SecretKey makes it.
type NewSecret struct {
	Key   string
	Value string
}

// ParseName checks a credential or rule name.
func ParseName(s string) (string, error) {
	if len(s) > 63 || !nameText.MatchString(s) {
		return "", fmt.Errorf("%w: name %q must be lower-case letters, digits, dots, dashes or underscores", ErrBadSettings, s)
	}
	return s, nil
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

// Matches is true when the request host is the pattern's host, or under its suffix.
func (p HostPattern) Matches(host string) bool {
	host = strings.ToLower(host)
	return host != "" && !strings.HasPrefix(host, "*") && p.Covers(HostPattern(host))
}

// HostPort is a rule's host with its port, 443 unless the rule names another.
type HostPort struct {
	Host HostPattern
	Port int
}

// DefaultPort is the port of a rule host that names none.
const DefaultPort = 443

// ParseHostPort reads host or host:port, with a host or *.suffix and a port of 1 to 65535.
func ParseHostPort(s string) (HostPort, error) {
	hostText, portText, hasPort := cutLast(s, ":")
	port := DefaultPort
	if hasPort {
		n, ok := digits(portText)
		if !ok || n < 1 || n > 65535 {
			return HostPort{}, fmt.Errorf("%w: host %q has a port that is not 1 to 65535", ErrBadSettings, s)
		}
		port = n
	}
	host, err := ParseHostPattern(hostText)
	if err != nil {
		return HostPort{}, err
	}
	return HostPort{Host: host, Port: port}, nil
}

// Matches is true for a request to host on port.
func (h HostPort) Matches(host string, port int) bool { return h.Port == port && h.Host.Matches(host) }

func (h HostPort) String() string {
	if h.Port == DefaultPort {
		return string(h.Host)
	}
	return string(h.Host) + ":" + strconv.Itoa(h.Port)
}

func digits(s string) (int, bool) {
	if s == "" {
		return 0, false
	}
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

// Validate refuses a document a parser could not have made: incomplete or repeated repositories, unnamed or
// repeated credentials and rules, and a rule that names a credential the document does not hold.
func (s Settings) Validate() error {
	seen := make(map[Repo]bool, len(s.Repositories))
	for _, r := range s.Repositories {
		if !r.Complete() || seen[r] {
			return fmt.Errorf("%w: incomplete or repeated repository %q", ErrBadSettings, r)
		}
		seen[r] = true
	}
	creds, err := s.validCredentials()
	if err != nil {
		return err
	}
	rules := make(map[string]bool, len(s.Rules))
	for _, r := range s.Rules {
		if err := r.validate(creds); err != nil || rules[r.Name] {
			return fmt.Errorf("%w: rule %q is repeated or incomplete", ErrBadSettings, r.Name)
		}
		rules[r.Name] = true
	}
	return nil
}

func (s Settings) validCredentials() (map[string]bool, error) {
	creds := make(map[string]bool, len(s.Credentials))
	for _, c := range s.Credentials {
		if err := c.validate(); err != nil || creds[c.Name] {
			return nil, fmt.Errorf("%w: credential %q is repeated or incomplete", ErrBadSettings, c.Name)
		}
		creds[c.Name] = true
	}
	return creds, nil
}

// Credential is the named credential, or false.
func (s Settings) Credential(name string) (Credential, bool) {
	for _, c := range s.Credentials {
		if c.Name == name {
			return c, true
		}
	}
	return Credential{}, false
}

// GitHubApp is the credential that holds the GitHub App, the first one, or false.
func (s Settings) GitHubApp() (Credential, bool) {
	for _, c := range s.Credentials {
		if c.App != nil {
			return c, true
		}
	}
	return Credential{}, false
}

// SecretKeys lists the key of every secret the document says is set, the ones a save keeps.
func (s Settings) SecretKeys() []string {
	var out []string
	for _, c := range s.Credentials {
		for _, f := range c.Fields() {
			if f.State == SecretSet {
				out = append(out, SecretKey(c.Name, f.Field))
			}
		}
	}
	return out
}
