package proxy

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"slices"
	"strings"

	"github.com/vektah/gqlparser/v2/ast"
	"github.com/vektah/gqlparser/v2/parser"

	"github.com/IniZio/moor/internal/domain"
)

// maxGraphQL is the largest GraphQL body the judge reads. A larger one is refused.
const maxGraphQL = 64 << 10

// mergeMessage is the reason every merge refusal gives, whatever the rules say.
const mergeMessage = "merging is not allowed from a task"

// envelope is the one JSON object a GraphQL request may be. Any other key, such as a persisted query's
// extensions, is refused.
type envelope struct {
	Query         string          `json:"query"`
	Variables     json.RawMessage `json:"variables"`
	OperationName string          `json:"operationName"`
}

// judgeGraphQL passes one query, or one mutation whose root fields, fragments expanded and judged by their real
// names, are all on mutations and none merges. It fails closed. The detail names GraphQL field names only.
func judgeGraphQL(body []byte, mutations []string) (domain.DenyReason, string, bool) {
	op, doc, why := readOperation(body)
	switch {
	case why != "":
		return domain.DenyGraphQL, why, false
	case op.Operation == ast.Query:
		return "", "", true
	case op.Operation != ast.Mutation:
		return domain.DenyGraphQL, "a subscription is not allowed", false
	}
	fields, err := rootFields(doc, op.SelectionSet, map[string]bool{})
	if err != nil {
		return domain.DenyGraphQL, err.Error(), false
	}
	if i := slices.IndexFunc(fields, merges); i >= 0 {
		return domain.DenyMerge, mergeMessage + " (mutation " + fields[i] + ")", false
	}
	var off []string
	for _, f := range fields {
		if f != "__typename" && !slices.Contains(mutations, f) {
			off = append(off, f)
		}
	}
	if len(off) > 0 {
		return domain.DenyGraphQL, "mutation " + strings.Join(off, ", ") + " is not on the rule's graphql_mutations list", false
	}
	return "", "", true
}

// readOperation reads the body as one envelope holding one operation. why says what is wrong with it.
func readOperation(body []byte) (*ast.OperationDefinition, *ast.QueryDocument, string) {
	trimmed := bytes.TrimSpace(body)
	switch {
	case len(body) > maxGraphQL:
		return nil, nil, "the GraphQL body is larger than 64 KiB"
	case bytes.HasPrefix(trimmed, []byte("[")):
		return nil, nil, "a batch of GraphQL requests is not allowed"
	}
	var env envelope
	dec := json.NewDecoder(bytes.NewReader(trimmed))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&env); err != nil || env.Query == "" || !errors.Is(dec.Decode(&struct{}{}), io.EOF) {
		return nil, nil, "the GraphQL envelope must be one object of query, variables and operationName"
	}
	doc, err := parser.ParseQuery(&ast.Source{Input: env.Query})
	if err != nil {
		return nil, nil, "the GraphQL document does not parse"
	}
	if len(doc.Operations) != 1 {
		return nil, nil, "a GraphQL request must hold one operation"
	}
	return doc.Operations[0], doc, ""
}

// rootFields lists the real names of the fields at the root of set, with its fragments expanded. seen holds the
// named fragments being expanded, so a cycle is refused.
func rootFields(doc *ast.QueryDocument, set ast.SelectionSet, seen map[string]bool) ([]string, error) {
	var out []string
	for _, sel := range set {
		if f, ok := sel.(*ast.Field); ok {
			out = append(out, f.Name)
			continue
		}
		more, err := fragmentFields(doc, sel, seen)
		if err != nil {
			return nil, err
		}
		out = append(out, more...)
	}
	return out, nil
}

// fragmentFields lists the root fields an inline fragment or a fragment spread selects.
func fragmentFields(doc *ast.QueryDocument, sel ast.Selection, seen map[string]bool) ([]string, error) {
	switch s := sel.(type) {
	case *ast.InlineFragment:
		return rootFields(doc, s.SelectionSet, seen)
	case *ast.FragmentSpread:
		frag := doc.Fragments.ForName(s.Name)
		if frag == nil || seen[s.Name] {
			return nil, fmt.Errorf("fragment %s is unknown or spreads itself", s.Name)
		}
		seen[s.Name] = true
		defer delete(seen, s.Name)
		return rootFields(doc, frag.SelectionSet, seen)
	}
	return nil, errors.New("a selection the judge does not know")
}

// merges is true for a mutation that merges, by its name or as a merge queue entry.
func merges(field string) bool {
	return strings.Contains(strings.ToLower(field), "merge") || field == "enqueuePullRequest"
}
