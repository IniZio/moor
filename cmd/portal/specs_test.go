package main

import (
	"net/http"
	"os"
	"path/filepath"
	"testing"
)

const prSpec = `name: PR evidence
on:
  pull_request:
    types: [opened, synchronize]
  workflow_dispatch:
prompt: Show evidence for ${{ event.pull_request.html_url }}.
`

const dependencySpec = `name: Dependency check
repositories:
  - example-org/shop-web
on:
  schedule:
    - cron: "0 6 * * 1"
prompt: Check the dependencies.
`

const otherRepoSpec = `name: Other repo
repositories:
  - example-org/other
on:
  workflow_dispatch:
prompt: p
`

// writeHome makes a checked-out team home with the given files under it and returns its folder.
func writeHome(t *testing.T, files map[string]string) string {
	t.Helper()
	dir := t.TempDir()
	for name, text := range files {
		path := filepath.Join(dir, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func withHome(dir string) func(*config) { return func(c *config) { c.homeDir = dir } }

func TestTaskSpecsAreListedFromTheHomeWithTheirProblems(t *testing.T) {
	home := writeHome(t, map[string]string{
		"tasks/pr-evidence.yaml":      prSpec,
		"tasks/dependency-check.yaml": dependencySpec,
		"tasks/other.yaml":            otherRepoSpec,
		"tasks/broken.yaml":           "name: x\non:\n  push:\nprompt: p\n",
		"tasks/notes.txt":             "ignored",
	})
	a := newApp(t, withHome(home))
	a.do(put("Mia", 0, goodDoc)).has(t, http.StatusOK)
	r := a.do(get("Mia", "/api/task-specs"))
	r.has(t, http.StatusOK,
		`"file":"tasks/pr-evidence.yaml"`, `"name":"PR evidence"`, `"manual":true`,
		`"starts":"Starts when a pull request is opened or updated on any repository in Settings. You can also start it by hand."`,
		`"name":"Dependency check"`, `"repositories":["example-org/shop-web"]`,
		`"starts":"Starts on the schedule 0 6 * * 1, one task per repository."`,
		`"file":"tasks/broken.yaml"`, `unknown key`,
		`"problem":"example-org/other is not under repositories in Settings. Add it there first."`)
	r.lacks(t, "notes.txt")
	a.do(get("", "/api/task-specs")).has(t, http.StatusUnauthorized)
}

func TestTheHomeFolderComesFromConfigurationAndHasNoDefault(t *testing.T) {
	if c, err := parseConfig(nil, env(goodEnv())); err != nil || c.homeDir != "" {
		t.Fatalf("default home = %q, %v", c.homeDir, err)
	}
	e := goodEnv()
	e["PORTAL_HOME_DIR"] = "/srv/home"
	if c, err := parseConfig(nil, env(e)); err != nil || c.homeDir != "/srv/home" {
		t.Fatalf("home from the environment = %q, %v", c.homeDir, err)
	}
	if c, err := parseConfig([]string{"-home-dir", "/other"}, env(e)); err != nil || c.homeDir != "/other" {
		t.Fatalf("home from the flag = %q, %v", c.homeDir, err)
	}
}

func TestWithoutAHomeThereAreNoTaskSpecs(t *testing.T) {
	a := newApp(t)
	a.do(get("Mia", "/api/task-specs")).has(t, http.StatusOK, `{"specs":[]}`)
}
