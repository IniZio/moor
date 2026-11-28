package domain

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"regexp"
	"strconv"
	"strings"
)

var ErrBadTemplate = errors.New("bad template")

// TemplateError names the line of the text (counted from 1) and the expression that failed.
type TemplateError struct {
	Line int
	Expr string
	Msg  string
}

func (e *TemplateError) Error() string {
	if e.Expr == "" {
		return fmt.Sprintf("%v: line %d: %s", ErrBadTemplate, e.Line, e.Msg)
	}
	return fmt.Sprintf("%v: line %d: ${{ %s }}: %s", ErrBadTemplate, e.Line, e.Expr, e.Msg)
}

func (e *TemplateError) Unwrap() error { return ErrBadTemplate }

type exprRoot string

const (
	rootEvent  exprRoot = "event"
	rootInputs exprRoot = "inputs"
)

type expression struct {
	text string
	line int
	root exprRoot
	path []string
}

type part struct {
	literal string
	expr    *expression
}

var pathSegment = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

func (e *expression) fail(msg string) *TemplateError {
	return &TemplateError{Line: e.line, Expr: e.text, Msg: msg}
}

// CheckTemplate reports the first expression that is not event.<path> or inputs.<name>, or that is not closed.
func CheckTemplate(text string) error {
	_, err := scanTemplate(text)
	return err
}

func scanTemplate(text string) ([]part, error) {
	var parts []part
	line := 1
	for text != "" {
		before, rest, found := strings.Cut(text, "${{")
		parts = append(parts, part{literal: before})
		line += strings.Count(before, "\n")
		if !found {
			break
		}
		body, after, closed := strings.Cut(rest, "}}")
		if !closed {
			return nil, &TemplateError{Line: line, Msg: "an expression opened with ${{ is not closed with }}"}
		}
		ex, err := parseExpression(body, line)
		if err != nil {
			return nil, err
		}
		parts = append(parts, part{expr: ex})
		line += strings.Count(body, "\n")
		text = after
	}
	return parts, nil
}

func parseExpression(body string, line int) (*expression, error) {
	text := strings.TrimSpace(body)
	ex := &expression{text: text, line: line}
	segs := strings.Split(text, ".")
	for _, s := range segs {
		if !pathSegment.MatchString(s) {
			return nil, ex.fail("only event.<path> and inputs.<name> are supported")
		}
	}
	ex.root, ex.path = exprRoot(segs[0]), segs[1:]
	if (ex.root == rootEvent && len(ex.path) >= 1) || (ex.root == rootInputs && len(ex.path) == 1) {
		return ex, nil
	}
	return nil, ex.fail("only event.<path> and inputs.<name> are supported")
}

// FillTemplate replaces each ${{ event.<path> }} with the string, number or boolean at that path of the
// payload, and each ${{ inputs.<name> }} with the input. Text outside expressions is kept byte for byte.
func FillTemplate(text string, payload json.RawMessage, inputs map[string]string) (string, error) {
	parts, err := scanTemplate(text)
	if err != nil {
		return "", err
	}
	r := resolver{payload: payload, inputs: inputs}
	var b strings.Builder
	for _, p := range parts {
		if p.expr == nil {
			b.WriteString(p.literal)
			continue
		}
		v, err := r.value(p.expr)
		if err != nil {
			return "", err
		}
		b.WriteString(v)
	}
	return b.String(), nil
}

type resolver struct {
	payload json.RawMessage
	inputs  map[string]string
	doc     map[string]any
}

func (r *resolver) value(ex *expression) (string, error) {
	if ex.root == rootInputs {
		v, ok := r.inputs[ex.path[0]]
		if !ok {
			return "", ex.fail("no such input")
		}
		return v, nil
	}
	if r.doc == nil {
		doc, err := decodeObject(r.payload)
		if err != nil {
			return "", ex.fail(err.Error())
		}
		r.doc = doc
	}
	var cur any = r.doc
	for _, seg := range ex.path {
		var ok bool
		if cur, ok = child(cur, seg); !ok {
			return "", ex.fail("no such path in the event")
		}
	}
	return scalarText(cur, ex)
}

func decodeObject(payload json.RawMessage) (map[string]any, error) {
	dec := json.NewDecoder(bytes.NewReader(payload))
	dec.UseNumber()
	var doc map[string]any
	if err := dec.Decode(&doc); err != nil || doc == nil {
		return nil, errors.New("the event payload is not a JSON object")
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return nil, errors.New("the event payload is not one JSON object")
	}
	return doc, nil
}

func child(cur any, seg string) (any, bool) {
	switch v := cur.(type) {
	case map[string]any:
		next, ok := v[seg]
		return next, ok
	case []any:
		i, err := strconv.Atoi(seg)
		if err != nil || i < 0 || i >= len(v) {
			return nil, false
		}
		return v[i], true
	}
	return nil, false
}

func scalarText(v any, ex *expression) (string, error) {
	switch t := v.(type) {
	case string:
		return t, nil
	case json.Number:
		return t.String(), nil
	case bool:
		return strconv.FormatBool(t), nil
	case map[string]any:
		return "", ex.fail("the value is an object, not text, a number or a boolean")
	case []any:
		return "", ex.fail("the value is an array, not text, a number or a boolean")
	}
	return "", ex.fail("the value is null")
}
