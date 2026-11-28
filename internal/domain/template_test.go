package domain_test

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

const prPayload = `{"action":"opened","number":42,"draft":false,"ratio":1.50,"big":12345678901234567890,
"pull_request":{"html_url":"https://forge.example/example-org/shop-web/pull/42","head":{"sha":"abc123"},
"labels":[{"name":"bug"},{"name":"ui"}],"body":null,"title":"a \"quoted\" ${{ title }}"},"empty":"","nested":{"x-y":{"z_1":"deep"}}}`

func TestFillTemplate(t *testing.T) {
	inputs := map[string]string{"env": "staging", "empty": ""}
	ok := []struct{ name, in, want string }{
		{"no expressions", "just text\n", "just text\n"},
		{"empty", "", ""},
		{"string", "see ${{ event.pull_request.html_url }}.", "see https://forge.example/example-org/shop-web/pull/42."},
		{"nested", "at ${{event.pull_request.head.sha}}!", "at abc123!"},
		{"spaces inside", "${{   event.pull_request.head.sha\t}}", "abc123"},
		{"number", "#${{ event.number }}", "#42"},
		{"float keeps its text", "${{ event.ratio }}", "1.50"},
		{"big number keeps its digits", "${{ event.big }}", "12345678901234567890"},
		{"boolean", "draft=${{ event.draft }}", "draft=false"},
		{"empty string", "[${{ event.empty }}]", "[]"},
		{"array index", "${{ event.pull_request.labels.1.name }}", "ui"},
		{"dashes and underscores", "${{ event.nested.x-y.z_1 }}", "deep"},
		{"value with expression text is not refilled", "${{ event.pull_request.title }}", "a \"quoted\" ${{ title }}"},
		{"input", "deploy to ${{ inputs.env }}", "deploy to staging"},
		{"empty input", "[${{ inputs.empty }}]", "[]"},
		{"two on one line", "${{ event.number }}/${{ inputs.env }}", "42/staging"},
		{"dollar signs", "cost $5, $$, ${HOME}, $(pwd), $", "cost $5, $$, ${HOME}, $(pwd), $"},
		{"braces", "{{ not an expression }} { } }} {{{", "{{ not an expression }} { } }} {{{"},
		{"dollar before an expression", "$${{ event.number }}", "$42"},
		{"shell and template syntax", "`id` {{.Title}} <% x %> ${ x }", "`id` {{.Title}} <% x %> ${ x }"},
		{"line endings", "a\r\n${{ event.number }}\r\n", "a\r\n42\r\n"},
		{"unicode", "審查 ${{ event.number }} ✓", "審查 42 ✓"},
	}
	for _, c := range ok {
		got, err := domain.FillTemplate(c.in, json.RawMessage(prPayload), inputs)
		if err != nil || got != c.want {
			t.Errorf("%s: got %q, %v, want %q", c.name, got, err, c.want)
		}
	}
}

func TestFillTemplateErrors(t *testing.T) {
	bad := []struct {
		name, in, payload string
		line              int
		expr, msg         string
	}{
		{"missing path", "x\n${{ event.pull_request.nope }}", prPayload, 2, "event.pull_request.nope", "no such path"},
		{"through a string", "${{ event.action.x }}", prPayload, 1, "event.action.x", "no such path"},
		{"array index out of range", "${{ event.pull_request.labels.2.name }}", prPayload, 1, "event.pull_request.labels.2.name", "no such path"},
		{"array index negative", "${{ event.pull_request.labels.-1.name }}", prPayload, 1, "event.pull_request.labels.-1.name", "no such path"},
		{"object value", "a\nb\n${{ event.pull_request.head }}", prPayload, 3, "event.pull_request.head", "object"},
		{"array value", "${{ event.pull_request.labels }}", prPayload, 1, "event.pull_request.labels", "array"},
		{"null value", "${{ event.pull_request.body }}", prPayload, 1, "event.pull_request.body", "null"},
		{"no payload", "${{ event.action }}", "", 1, "event.action", "JSON object"},
		{"payload array", "${{ event.action }}", "[1]", 1, "event.action", "JSON object"},
		{"payload null", "${{ event.action }}", "null", 1, "event.action", "JSON object"},
		{"payload two values", "${{ event.action }}", "{} {}", 1, "event.action", "one JSON object"},
		{"payload broken", "${{ event.action }}", "{", 1, "event.action", "JSON object"},
		{"missing input", "${{ inputs.nope }}", "{}", 1, "inputs.nope", "no such input"},
		{"event alone", "${{ event }}", "{}", 1, "event", "supported"},
		{"inputs alone", "${{ inputs }}", "{}", 1, "inputs", "supported"},
		{"inputs with a path", "${{ inputs.a.b }}", "{}", 1, "inputs.a.b", "supported"},
		{"secrets", "\n\n${{ secrets.TOKEN }}", "{}", 3, "secrets.TOKEN", "supported"},
		{"github context", "${{ github.sha }}", "{}", 1, "github.sha", "supported"},
		{"function call", "${{ format('x') }}", "{}", 1, "format('x')", "supported"},
		{"operator", "${{ event.a || event.b }}", "{}", 1, "event.a || event.b", "supported"},
		{"index syntax", "${{ event.labels[0] }}", "{}", 1, "event.labels[0]", "supported"},
		{"empty expression", "${{ }}", "{}", 1, "", "supported"},
		{"expression over two lines", "${{ event.\naction }}", "{}", 1, "event.\naction", "supported"},
		{"not closed", "ok\nbroken ${{ event.action", "{}", 2, "", "not closed"},
		{"line counts a multi-line expression", "${{ event.\naction }}\n${{ x }}", "{}", 1, "event.\naction", "supported"},
	}
	for _, c := range bad {
		_, err := domain.FillTemplate(c.in, json.RawMessage(c.payload), map[string]string{})
		var te *domain.TemplateError
		if !errors.As(err, &te) || !errors.Is(err, domain.ErrBadTemplate) {
			t.Errorf("%s: err = %v", c.name, err)
			continue
		}
		if te.Line != c.line || te.Expr != c.expr || !strings.Contains(te.Msg, c.msg) {
			t.Errorf("%s: got line %d expr %q msg %q, want line %d expr %q msg containing %q", c.name, te.Line, te.Expr, te.Msg, c.line, c.expr, c.msg)
		}
	}
}

func TestCheckTemplateNeedsNoPayload(t *testing.T) {
	if err := domain.CheckTemplate("a ${{ event.x.y }} b ${{ inputs.z }} $5 {{x}}"); err != nil {
		t.Fatal(err)
	}
	if err := domain.CheckTemplate("a ${{ env.X }}"); !errors.Is(err, domain.ErrBadTemplate) {
		t.Fatalf("err = %v", err)
	}
}

func TestSpecFillsBothPrompts(t *testing.T) {
	s := fullSpec()
	s.Prompt, s.ReviewPrompt = "p ${{ event.action }}", "r ${{ inputs.env }}"
	got, err := s.Fill(json.RawMessage(`{"action":"opened"}`), map[string]string{"env": "dev"})
	if err != nil || got.Prompt != "p opened" || got.ReviewPrompt != "r dev" {
		t.Fatalf("got %+v, %v", got, err)
	}
	s.ReviewPrompt = "${{ inputs.nope }}"
	if _, err := s.Fill(json.RawMessage(`{"action":"opened"}`), nil); !errors.Is(err, domain.ErrBadTemplate) || !strings.Contains(err.Error(), "review") {
		t.Fatalf("err = %v", err)
	}
}
