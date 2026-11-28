package yamlfile_test

import (
	"errors"
	"reflect"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
)

func yml(lines ...string) []byte { return []byte(strings.Join(lines, "\n") + "\n") }

var exampleSpec = yml(
	"name: PR evidence",
	"repositories:            # optional; without it, every allowed repository",
	"  - example-org/shop-web",
	"on:",
	"  pull_request:",
	"    types: [opened, synchronize, reopened, ready_for_review]",
	"  schedule:",
	`    - cron: "0 6 * * 1"`,
	"  workflow_dispatch:",
	"concurrency:",
	"  cancel-in-progress: true",
	"prompt: |",
	"  Show evidence for ${{ event.pull_request.html_url }}",
	"  at ${{ event.pull_request.head.sha }}.",
	"review: |",
	"  Check your page, fix what fails, then publish again.",
)

func TestParseTaskSpecExample(t *testing.T) {
	got, err := yamlfile.ParseTaskSpec(exampleSpec)
	if err != nil {
		t.Fatal(err)
	}
	want := domain.TaskSpec{
		Name:         "PR evidence",
		Repositories: []domain.Repo{{Host: "github.com", Path: "example-org/shop-web"}},
		On: domain.On{
			PullRequest:      &domain.PullRequestOn{Types: []domain.PullRequestType{domain.PROpened, domain.PRSynchronize, domain.PRReopened, domain.PRReadyForReview}},
			Schedule:         []domain.ScheduleOn{{Cron: "0 6 * * 1"}},
			WorkflowDispatch: &domain.DispatchOn{},
		},
		Concurrency:  domain.Concurrency{CancelInProgress: true},
		Prompt:       "Show evidence for ${{ event.pull_request.html_url }}\nat ${{ event.pull_request.head.sha }}.\n",
		ReviewPrompt: "Check your page, fix what fails, then publish again.\n",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got  %+v\nwant %+v", got, want)
	}
	filled, err := got.Fill([]byte(`{"pull_request":{"html_url":"https://forge.example/pull/7","head":{"sha":"abc"}}}`), nil)
	if err != nil || filled.Prompt != "Show evidence for https://forge.example/pull/7\nat abc.\n" {
		t.Fatalf("filled %q, %v", filled.Prompt, err)
	}
}

func TestParseTaskSpecMinimalAndOptions(t *testing.T) {
	got, err := yamlfile.ParseTaskSpec(yml("name: a", "on:", "  pull_request:", "prompt: p"))
	want := domain.TaskSpec{Name: "a", On: domain.On{PullRequest: &domain.PullRequestOn{}}, Prompt: "p"}
	if err != nil || !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v, %v", got, err)
	}
	got, err = yamlfile.ParseTaskSpec(yml(
		"name: 審查",
		"repositories: [forge.example/team/sub/app, Example-Org/Shop-Web]",
		"on:",
		"  workflow_dispatch:",
		"    inputs:",
		"      env:",
		"        description: Where to run",
		"        default: staging",
		"        required: true",
		"      verbose:",
		"      count:",
		"        default: 3",
		"  schedule:",
		"    - cron: '*/5 9-17 * * mon-fri'",
		"    - cron: 0 0 1 jan *",
		"concurrency:",
		"  cancel-in-progress: false",
		"prompt: >",
		"  one",
		"  two",
	))
	staging, three := "staging", "3"
	want = domain.TaskSpec{
		Name:         "審查",
		Repositories: []domain.Repo{{Host: "forge.example", Path: "team/sub/app"}, {Host: "github.com", Path: "example-org/shop-web"}},
		On: domain.On{
			Schedule: []domain.ScheduleOn{{Cron: "*/5 9-17 * * mon-fri"}, {Cron: "0 0 1 jan *"}},
			WorkflowDispatch: &domain.DispatchOn{Inputs: []domain.Input{
				{Name: "env", Description: "Where to run", Default: &staging, Required: true},
				{Name: "verbose"},
				{Name: "count", Default: &three},
			}},
		},
		Prompt: "one two\n",
	}
	if err != nil || !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v, %v\nwant %+v", got, err, want)
	}
}

const skeleton = "name: a\non:\n  workflow_dispatch:\nprompt: p\n"

type errorCase struct {
	name string
	in   string
	line int
	msg  string
}

var specErrorCases = []errorCase{
	{"empty file", "", 1, "empty"},
	{"comment only", "# nothing\n", 1, "empty"},
	{"not a mapping", "- a\n", 1, "must be a mapping"},
	{"unknown top key", skeleton + "branches: [x]\n", 5, `unknown key "branches"`},
	{"unknown event", "name: a\non:\n  push:\nprompt: p\n", 3, `unknown key "push"`},
	{"unknown pull_request key", "name: a\non:\n  pull_request:\n    branches: [main]\nprompt: p\n", 4, `unknown key "branches"`},
	{"unknown schedule key", "name: a\non:\n  schedule:\n    - cron: '* * * * *'\n      tz: x\nprompt: p\n", 5, `unknown key "tz"`},
	{"unknown concurrency key", skeleton + "concurrency:\n  group: x\n", 6, `unknown key "group"`},
	{"unknown input key", "name: a\non:\n  workflow_dispatch:\n    inputs:\n      env:\n        type: choice\nprompt: p\n", 6, `unknown key "type"`},
	{"unknown dispatch key", "name: a\non:\n  workflow_dispatch:\n    branches: x\nprompt: p\n", 4, `unknown key "branches"`},
	{"repeated key", "name: a\nname: b\non:\n  workflow_dispatch:\nprompt: p\n", 2, `"name" is repeated`},
	{"key that is not text", "[a]: b\n", 1, "not text"},
	{"name is a number", "name: 12\non:\n  workflow_dispatch:\nprompt: p\n", 1, "name must be text"},
	{"name is a list", "name: [a]\non:\n  workflow_dispatch:\nprompt: p\n", 1, "name must be text"},
	{"name is null", "name:\non:\n  workflow_dispatch:\nprompt: p\n", 1, "name must be text"},
	{"name has two lines", "name: \"a\\nb\"\non:\n  workflow_dispatch:\nprompt: p\n", 1, "one line"},
	{"name is blank", "name: ''\non:\n  workflow_dispatch:\nprompt: p\n", 1, "one line"},
	{"missing name", "on:\n  workflow_dispatch:\nprompt: p\n", 1, `missing required key "name"`},
	{"missing on", "name: a\nprompt: p\n", 1, `missing required key "on"`},
	{"missing prompt", "name: a\non:\n  workflow_dispatch:\n", 1, `missing required key "prompt"`},
	{"on without events", "name: a\non: {}\nprompt: p\n", 2, "at least one event"},
	{"on is empty", "name: a\non:\nprompt: p\n", 2, "on must be a mapping"},
	{"on is text", "name: a\non: push\nprompt: p\n", 2, "on must be a mapping"},
	{"on is a list", "name: a\non: [push]\nprompt: p\n", 2, "on must be a mapping"},
	{"cron with too few fields", "name: a\non:\n  schedule:\n    - cron: '* * * *'\nprompt: p\n", 4, "cron"},
	{"cron out of range", "name: a\non:\n  schedule:\n    - cron: '61 * * * *'\nprompt: p\n", 4, "above maximum"},
	{"cron is a number", "name: a\non:\n  schedule:\n    - cron: 5\nprompt: p\n", 4, "cron must be text"},
	{"second cron is bad", "name: a\non:\n  schedule:\n    - cron: '* * * * *'\n    - cron: nope\nprompt: p\n", 5, "cron"},
	{"schedule entry without cron", "name: a\non:\n  schedule:\n    - {}\nprompt: p\n", 4, `missing required key "cron"`},
	{"schedule is text", "name: a\non:\n  schedule: nightly\nprompt: p\n", 3, "schedule must be a list"},
	{"schedule is empty", "name: a\non:\n  schedule: []\nprompt: p\n", 3, "at least one cron"},
	{"schedule entry is text", "name: a\non:\n  schedule:\n    - nightly\nprompt: p\n", 4, "must be a mapping"},
	{"unknown pull_request type", "name: a\non:\n  pull_request:\n    types: [opened, merged]\nprompt: p\n", 4, `unknown pull_request type "merged"`},
	{"repeated pull_request type", "name: a\non:\n  pull_request:\n    types:\n      - opened\n      - opened\nprompt: p\n", 6, "listed twice"},
	{"types is text", "name: a\non:\n  pull_request:\n    types: opened\nprompt: p\n", 4, "types must be a list"},
	{"types is empty", "name: a\non:\n  pull_request:\n    types: []\nprompt: p\n", 4, "types is empty"},
	{"type is a number", "name: a\non:\n  pull_request:\n    types: [1]\nprompt: p\n", 4, "must be text"},
	{"repository without owner", "name: a\nrepositories:\n  - example-org/shop-web\n  - shop-web\non:\n  workflow_dispatch:\nprompt: p\n", 4, "shop-web"},
	{"repository with a scheme", "name: a\nrepositories: [https://github.com/a/b]\non:\n  workflow_dispatch:\nprompt: p\n", 2, "https"},
	{"repository listed twice", "name: a\nrepositories:\n  - example-org/shop\n  - github.com/Example-Org/Shop\non:\n  workflow_dispatch:\nprompt: p\n", 4, "listed twice"},
	{"repositories is text", "name: a\nrepositories: a/b\non:\n  workflow_dispatch:\nprompt: p\n", 2, "must be a list"},
	{"repositories is empty", "name: a\nrepositories: []\non:\n  workflow_dispatch:\nprompt: p\n", 2, "repositories is empty"},
	{"repository is a map", "name: a\nrepositories:\n  - {a: b}\non:\n  workflow_dispatch:\nprompt: p\n", 3, "must be text"},
	{"prompt is a number", "name: a\non:\n  workflow_dispatch:\nprompt: 5\n", 4, "prompt must be text"},
	{"prompt is a list", "name: a\non:\n  workflow_dispatch:\nprompt: [a]\n", 4, "prompt must be text"},
	{"prompt is blank", "name: a\non:\n  workflow_dispatch:\nprompt: '  '\n", 4, "prompt is empty"},
	{"review is a number", skeleton + "review: 1\n", 5, "review must be text"},
	{"expression in a literal block", "name: a\non:\n  workflow_dispatch:\nprompt: |\n  first\n  ${{ secrets.X }}\n", 6, "secrets.X"},
	{"expression on the first line of a block", "name: a\non:\n  workflow_dispatch:\nprompt: |\n  ${{ env.X }}\n", 5, "env.X"},
	{"expression in a quoted prompt", "name: a\non:\n  workflow_dispatch:\nprompt: \"go ${{ x.y }}\"\n", 4, "x.y"},
	{"expression not closed", "name: a\non:\n  workflow_dispatch:\nprompt: p\nreview: |\n  ok\n  ${{ event.a\n", 7, "not closed"},
	{"expression in review", skeleton + "review: ${{ inputs }}\n", 5, "inputs"},
	{"cancel-in-progress is not a flag", skeleton + "concurrency:\n  cancel-in-progress: maybe\n", 6, "true or false"},
	{"concurrency is text", skeleton + "concurrency: a\n", 5, "must be a mapping"},
	{"inputs is a list", "name: a\non:\n  workflow_dispatch:\n    inputs: [a]\nprompt: p\n", 4, "inputs must be a mapping"},
	{"input is text", "name: a\non:\n  workflow_dispatch:\n    inputs:\n      env: staging\nprompt: p\n", 5, "must be a mapping"},
	{"input name with a dot", "name: a\non:\n  workflow_dispatch:\n    inputs:\n      a.b:\nprompt: p\n", 5, "a.b"},
	{"input required is text", "name: a\non:\n  workflow_dispatch:\n    inputs:\n      env:\n        required: sure\nprompt: p\n", 6, "true or false"},
	{"input default is a list", "name: a\non:\n  workflow_dispatch:\n    inputs:\n      env:\n        default: [a]\nprompt: p\n", 6, "single value"},
	{"input description is a number", "name: a\non:\n  workflow_dispatch:\n    inputs:\n      env:\n        description: 4\nprompt: p\n", 6, "must be text"},
	{"alias", "name: &x a\non:\n  workflow_dispatch:\nprompt: *x\n", 4, "aliases"},
	{"two documents", skeleton + "---\nname: b\n", 5, "second YAML document"},
	{"syntax error", "name: a\non: [\n", 2, "expected node content"},
	{"tab indentation", "name: a\non:\n\tworkflow_dispatch:\nprompt: p\n", 3, "cannot start any token"},
	{"too large", "name: a\n# " + strings.Repeat("x", 1<<20) + "\n", 0, "1 MiB"},
}

func TestParseTaskSpecErrors(t *testing.T) {
	for _, c := range specErrorCases {
		_, err := yamlfile.ParseTaskSpec([]byte(c.in))
		var pe yamlfile.ParseErrors
		if !errors.As(err, &pe) || !errors.Is(err, yamlfile.ErrInvalid) || len(pe) == 0 {
			t.Errorf("%s: err = %v", c.name, err)
			continue
		}
		if pe[0].Line != c.line || !strings.Contains(pe[0].Msg, c.msg) {
			t.Errorf("%s: got line %d %q, want line %d containing %q", c.name, pe[0].Line, pe[0].Msg, c.line, c.msg)
		}
	}
}

func TestParseTaskSpecReportsEveryProblem(t *testing.T) {
	_, err := yamlfile.ParseTaskSpec(yml(
		"name: 5",
		"on:",
		"  push:",
		"  schedule:",
		"    - cron: nope",
		"prompt: ${{ x }}",
		"extra: 1",
	))
	var pe yamlfile.ParseErrors
	if !errors.As(err, &pe) {
		t.Fatal(err)
	}
	var lines []int
	for _, p := range pe {
		lines = append(lines, p.Line)
	}
	if want := []int{3, 7, 1, 5, 6}; !reflect.DeepEqual(sorted(lines), sorted(want)) {
		t.Fatalf("lines %v from %v, want %v", lines, err, want)
	}
	if !strings.Contains(err.Error(), "line 5") || !strings.Contains(err.Error(), "line 7") {
		t.Fatalf("message %q", err)
	}
}

func sorted(in []int) []int {
	out := append([]int(nil), in...)
	for i := range out {
		for j := i + 1; j < len(out); j++ {
			if out[j] < out[i] {
				out[i], out[j] = out[j], out[i]
			}
		}
	}
	return out
}
