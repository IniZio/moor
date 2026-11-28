package main

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"
)

const imageArgs = "-box-image registry.example.org/box:1 -desk-image registry.example.org/desk:1 -relay-image registry.example.org/relay:1 -proxy-image registry.example.org/proxy:1"

func TestTheJoinCommandCarriesTheOperatorsRunnerArgsOnTheSameLine(t *testing.T) {
	a := newApp(t, withPublicURL, func(c *config) { c.runnerArgs = imageArgs })
	r := a.do(req{method: http.MethodPost, path: "/api/machines", person: "Mia"})
	r.has(t, http.StatusCreated)
	var out struct {
		JoinCommand string `json:"join_command"`
	}
	if err := json.Unmarshal([]byte(r.body), &out); err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(out.JoinCommand, "runner join -portal https://portal.example.com -id m-") || !strings.HasSuffix(out.JoinCommand, " -key runner.key "+imageArgs) || strings.Contains(out.JoinCommand, "\n") {
		t.Fatalf("join command = %q", out.JoinCommand)
	}
}

func TestRunnerArgsComeFromAFlagOrItsVariableAndStayOnOneLine(t *testing.T) {
	e := goodEnv()
	e["PORTAL_RUNNER_ARGS"] = "-box-image a"
	if c, err := parseConfig(nil, env(e)); err != nil || c.runnerArgs != "-box-image a" {
		t.Fatalf("from the variable: %q, %v", c.runnerArgs, err)
	}
	if c, err := parseConfig([]string{"-runner-args", "-box-image b"}, env(e)); err != nil || c.runnerArgs != "-box-image b" {
		t.Fatalf("from the flag: %q, %v", c.runnerArgs, err)
	}
	if _, err := parseConfig([]string{"-runner-args", "-box-image b\nrm -rf /"}, env(goodEnv())); err == nil {
		t.Error("runner args with a line break were accepted")
	}
	if c, err := parseConfig(nil, env(goodEnv())); err != nil || c.runnerArgs != "" {
		t.Errorf("by default: %q, %v", c.runnerArgs, err)
	}
}
