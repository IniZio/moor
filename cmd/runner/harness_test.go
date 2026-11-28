package main

import (
	"maps"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/wire"
)

func TestTheHarnessFlagNamesALauncherAndTheServiceKeepsIt(t *testing.T) {
	base := []string{"-portal", "ws://p.invalid/runner/link", "-id", "m-1", "-key", "k"}
	o, err := parse(append(append([]string{}, base...), "-harness", "agent-2"))
	if err != nil || o.harness != "agent-2" {
		t.Fatalf("parse: %v, harness %q", err, o.harness)
	}
	u, err := unitFor("/x/runner", o)
	if err != nil || !strings.HasSuffix(strings.Join(u.argv, " "), " -harness agent-2") {
		t.Fatalf("the service runs %q: %v", u.argv, err)
	}
	for _, bad := range []string{"../standin", "Agent", "a b", "-x"} {
		if _, err := parse(append(append([]string{}, base...), "-harness", bad)); err == nil {
			t.Errorf("-harness %q was taken", bad)
		}
	}
	o, _ = parse(base)
	if u, _ := unitFor("/x/runner", o); strings.Contains(strings.Join(u.argv, " "), "-harness") {
		t.Errorf("a runner with no harness set names one: %q", u.argv)
	}
}

func TestTheSessionGetsTheDeskVariablesThePromptAndTheLauncher(t *testing.T) {
	s := wire.TaskStart{Prompt: "fix the build", DeskEnv: []wire.EnvVar{{Name: "MODEL_TOKEN", Value: "moor-placeholder-model"}, {Name: "TASK_PROMPT", Value: "not this"}}}
	got := sessionEnv(s, "agent-2")
	want := map[string]string{"MODEL_TOKEN": "moor-placeholder-model", "TASK_PROMPT": "fix the build", "MOOR_HARNESS": "agent-2"}
	if !maps.Equal(got, want) {
		t.Fatalf("session env %v, want %v", got, want)
	}
	if _, ok := sessionEnv(s, "")["MOOR_HARNESS"]; ok {
		t.Errorf("no harness set still names one, so the image's default is not taken")
	}
}
