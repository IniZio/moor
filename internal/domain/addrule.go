package domain

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"unicode"
)

// AddRule says how a connection puts its secret into a request. HeaderAdd and PlaceholderAdd are the only ones.
type AddRule interface {
	String() string
	isAddRule()
}

// HeaderAdd sets a header to Template with {value} replaced by the secret.
type HeaderAdd struct {
	Name     string
	Template string
}

// PlaceholderAdd replaces Token wherever the request carries it with the secret.
type PlaceholderAdd struct {
	Token string
}

const valueMark = "{value}"

var (
	headerName  = regexp.MustCompile(`^[A-Za-z0-9-]+$`)
	tokenName   = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
	errBadAdd   = fmt.Errorf("%w: add rule", ErrBadSettings)
	addGrammar  = `header <Name> "<template containing {value}>" or placeholder <TOKEN>`
	badTemplate = "the header template has no {value}, is not a quoted string, or has a control character"
)

func (HeaderAdd) isAddRule()      {}
func (PlaceholderAdd) isAddRule() {}

func (h HeaderAdd) String() string      { return "header " + h.Name + " " + strconv.Quote(h.Template) }
func (p PlaceholderAdd) String() string { return "placeholder " + p.Token }

// ParseAddRule reads header <Name> "<template containing {value}>" or placeholder <TOKEN>.
func ParseAddRule(s string) (AddRule, error) {
	kind, rest, _ := strings.Cut(strings.TrimSpace(s), " ")
	rest = strings.TrimSpace(rest)
	switch kind {
	case "header":
		name, quoted, _ := strings.Cut(rest, " ")
		tmpl, err := strconv.Unquote(strings.TrimSpace(quoted))
		if !headerName.MatchString(name) || err != nil || !strings.Contains(tmpl, valueMark) || strings.ContainsFunc(tmpl, unicode.IsControl) {
			return nil, fmt.Errorf("%w %q: %s", errBadAdd, s, badTemplate)
		}
		return HeaderAdd{Name: name, Template: tmpl}, nil
	case "placeholder":
		if tokenName.MatchString(rest) {
			return PlaceholderAdd{Token: rest}, nil
		}
	}
	return nil, fmt.Errorf("%w %q is not %s", errBadAdd, s, addGrammar)
}
