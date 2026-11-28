package main

import (
	"net/http"
	"os"
	"regexp"
	"slices"
	"strings"
	"testing"

	"go.yaml.in/yaml/v3"
)

// machineCalls are the requests that reach the portal without a person: the GitHub webhook, a runner's join and
// link, and the proxy and desk of a task. They are what the chart's Ingress must route and nothing more.
var machineCalls = []struct{ method, path string }{
	{http.MethodPost, "/hooks/github"},
	{http.MethodPost, "/runner/join"},
	{http.MethodGet, "/runner/link"},
	{http.MethodGet, "/internal/policy/t1"},
	{http.MethodPost, "/internal/credentials/value"},
	{http.MethodPost, "/internal/blocked"},
	{http.MethodPost, "/api/tasks/t1/results"},
}

type chartIngress struct {
	Ingress struct {
		Paths    []string `yaml:"paths"`
		Patterns []string `yaml:"patterns"`
	} `yaml:"ingress"`
}

func chartDefaults(t *testing.T) chartIngress {
	t.Helper()
	text, err := os.ReadFile("../../deploy/chart/values.yaml")
	if err != nil {
		t.Fatal(err)
	}
	var v chartIngress
	if err := yaml.Unmarshal(text, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func (c chartIngress) routes(path string) bool {
	for _, p := range c.Ingress.Paths {
		if path == p || strings.HasPrefix(path, p+"/") {
			return true
		}
	}
	for _, p := range c.Ingress.Patterns {
		if regexp.MustCompile("^" + p + "$").MatchString(path) {
			return true
		}
	}
	return false
}

func TestTheChartsMachinePathsAreTheRoutesTheMachinesCall(t *testing.T) {
	a := newApp(t, func(c *config) { c.publicURL = "http://portal.example.org" })
	for _, call := range machineCalls {
		if got := a.do(req{method: call.method, path: call.path}); got.code == http.StatusNotFound {
			t.Errorf("%s %s is not a route of the portal: the table of machine calls is stale", call.method, call.path)
		}
	}
	chart := chartDefaults(t)
	for _, call := range machineCalls {
		if !chart.routes(call.path) {
			t.Errorf("%s %s is called by a machine but not routed by the chart's default ingress paths", call.method, call.path)
		}
	}
	for _, p := range append(slices.Clone(chart.Ingress.Paths), chart.Ingress.Patterns...) {
		if !calledBy(p) {
			t.Errorf("the chart's default ingress routes %q, which no machine calls", p)
		}
	}
}

func calledBy(entry string) bool {
	one := chartIngress{}
	one.Ingress.Paths, one.Ingress.Patterns = []string{entry}, []string{entry}
	return slices.ContainsFunc(machineCalls, func(call struct{ method, path string }) bool { return one.routes(call.path) })
}
