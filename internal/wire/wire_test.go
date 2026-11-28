package wire_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/wire"
)

var fence = wire.Fence{Task: "t1", Run: 2, Gen: 3}

var repo = wire.Repo{Host: "forge.example", Path: "acme/app"}

var messages = []struct {
	name   string
	msg    any
	fields string
}{
	{"error", &wire.Error{Code: wire.CodeBusy, Msg: "too many ops"}, "code msg"},
	{"hello", &wire.Hello{Runner: "r1", Proto: wire.Proto, Build: "b1"}, "build proto runner"},
	{"challenge", &wire.Challenge{Nonce: []byte{1, 2}}, "nonce"},
	{"proof", &wire.Proof{Sig: []byte{3}}, "sig"},
	{"welcome", &wire.Welcome{Proto: wire.Proto, Gen: 4}, "gen proto"},
	{"update_required", &wire.UpdateRequired{ServerProto: 2, MinProto: 2}, "min_proto server_proto"},
	{"refused", &wire.Refused{Reason: "unknown runner"}, "reason"},
	{"relink", &wire.Relink{}, ""},
	{"runner.status", &wire.RunnerStatus{}, ""},
	{"runner.status reply", &wire.RunnerStatusReply{Build: "b1", Proto: 1, Slots: 4, Tasks: 1}, "build proto slots tasks"},
	{"runner.report", &wire.RunnerReport{DiskFree: 10, MemFree: 20, SlotsFree: 3}, "disk_free mem_free slots_free"},
	{"task.start", &wire.TaskStart{
		Fence: fence, Repo: repo, Commit: "abc", Setup: "make setup", Prompt: "show it", ReviewPrompt: "check it", Skills: []string{"drive"},
		Spec:      &wire.Spec{Name: "pr-evidence", File: "specs/pr-evidence.yaml"},
		Event:     &wire.Event{Source: "forge", Group: "acme/app#7", Rev: "abc", Context: json.RawMessage(`{"n":7}`)},
		Harness:   "claude",
		WarmState: "w1", BudgetSeconds: 3600, ModelToken: "tok", AllowHosts: []string{"example.com"},
		SecretRules: json.RawMessage(`[{"host":"example.com"}]`),
	}, "allow_hosts budget commit event gen harness model_token prompt repo review_prompt run secret_rules setup skills spec task warm_state"},
	{"task.watch", &wire.TaskWatch{}, ""},
	{"task.watch line", &wire.TaskWatchLine{Fence: fence, Part: "box", State: "up", Code: 0}, "code gen part run state task"},
	{"box.recreate", &wire.BoxRecreate{Fence: fence, HomeCommit: "h1", WarmState: "w1"}, "gen home_commit run task warm_state"},
	{"task.stop", &wire.TaskStop{Fence: fence}, "gen run task"},
	{"task.release", &wire.TaskRelease{Task: "t1"}, "task"},
	{"task.list", &wire.TaskList{}, ""},
	{"task.list reply", &wire.TaskListReply{Tasks: []wire.ListedTask{{Fence: fence, Containers: []string{"desk"}}}}, "tasks"},
	{"term.attach", &wire.TermAttach{Task: "t1", Mode: "steer", Cols: 80, Rows: 24, History: 1000}, "cols history mode rows task"},
	{"warm.build", &wire.WarmBuild{Repo: repo, Commit: "abc", Setup: "make setup", InputHash: "i1", HomeCommit: "h1", BudgetSeconds: 900}, "budget commit home_commit input_hash repo setup"},
}

func TestMessagesRoundTrip(t *testing.T) {
	for _, m := range messages {
		data, err := json.Marshal(m.msg)
		if err != nil {
			t.Fatalf("%s: %v", m.name, err)
		}
		back := reflect.New(reflect.TypeOf(m.msg).Elem()).Interface()
		dec := json.NewDecoder(bytes.NewReader(data))
		dec.DisallowUnknownFields()
		if err := dec.Decode(back); err != nil {
			t.Fatalf("%s: %v", m.name, err)
		}
		if !reflect.DeepEqual(back, m.msg) {
			t.Errorf("%s: %+v came back as %+v", m.name, m.msg, back)
		}
		if got := fieldNames(t, data); got != m.fields {
			t.Errorf("%s: fields %q, want %q", m.name, got, m.fields)
		}
	}
}

func TestErrorMatchesOnCodeAlone(t *testing.T) {
	err := fmt.Errorf("call: %w", &wire.Error{Code: wire.CodeRefused, Msg: "superseded"})
	if !errors.Is(err, &wire.Error{Code: wire.CodeRefused}) {
		t.Error("an error of the same code must match")
	}
	if errors.Is(err, &wire.Error{Code: wire.CodeBusy}) {
		t.Error("an error of another code must not match")
	}
}

func TestTaskStartOmitsTheOptionalFieldsOnAPlainPrompt(t *testing.T) {
	data, err := json.Marshal(wire.TaskStart{Fence: fence, Prompt: "fix it"})
	if err != nil {
		t.Fatal(err)
	}
	for _, optional := range []string{"spec", "event", "review_prompt"} {
		if got := fieldNames(t, data); strings.Contains(got, optional) {
			t.Errorf("fields %q", got)
		}
	}
}

func fieldNames(t *testing.T, data []byte) string {
	t.Helper()
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		t.Fatal(err)
	}
	names := make([]string, 0, len(fields))
	for name := range fields {
		names = append(names, name)
	}
	slices.Sort(names)
	return strings.Join(names, " ")
}
