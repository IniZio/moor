package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

var answerable = map[domain.EventSource]bool{"forge": true}

func evented(spec domain.SpecName, source domain.EventSource) domain.Task {
	return domain.Task{
		Project: "app", Spec: &domain.Spec{Name: spec, File: "specs/" + string(spec) + ".yaml"},
		Event: &domain.Event{Source: source, Group: "acme/app#7", Rev: "abc", Context: []byte(`{"n":7}`)},
	}
}

func TestTaskKey(t *testing.T) {
	first, redelivered := evented("review", "forge"), evented("review", "forge")
	first.ID, redelivered.ID = "t1", "t2"
	k1, ok1 := first.Key()
	k2, ok2 := redelivered.Key()
	if !ok1 || !ok2 || k1 != k2 {
		t.Fatalf("keys %v %v, %v %v", k1, ok1, k2, ok2)
	}
	other, ok := evented("nightly", "forge").Key()
	if !ok || other == k1 || other.Spec != "nightly" {
		t.Errorf("two specs on one event share the key %v and %v", k1, other)
	}
	if _, ok := (domain.Task{ID: "t3", Project: "app"}).Key(); ok {
		t.Error("a plain-prompt task has a key")
	}
	noSpec := evented("review", "forge")
	noSpec.Spec = nil
	if _, ok := noSpec.Key(); ok {
		t.Error("an event without a spec has a key")
	}
}

func TestFlowReadsTheEventAndItsSource(t *testing.T) {
	cases := map[string]struct {
		task domain.Task
		want domain.Flow
	}{
		"plain prompt":                {domain.Task{}, domain.Plain},
		"event of an answered source": {evented("review", "forge"), domain.Answering},
		"event of another source":     {evented("nightly", "schedule"), domain.Evented},
	}
	for name, c := range cases {
		if got := c.task.Flow(answerable); got != c.want {
			t.Errorf("%s: flow = %s, want %s", name, got, c.want)
		}
	}
	if got := evented("review", "forge").Flow(nil); got != domain.Evented {
		t.Errorf("no answerable sources: flow = %s", got)
	}
}

func TestAnswers(t *testing.T) {
	run := evented("review", "manual")
	run.StartedBy = "ann"
	cases := map[string]struct {
		task domain.Task
		want bool
	}{
		"plain prompt":            {domain.Task{StartedBy: "ann"}, false},
		"answerable source":       {evented("review", "forge"), true},
		"source not answerable":   {evented("nightly", "schedule"), false},
		"run by hand, not listed": {run, false},
	}
	for name, c := range cases {
		if got := c.task.Answers(answerable); got != c.want {
			t.Errorf("%s: Answers = %v, want %v", name, got, c.want)
		}
	}
}

func TestNewProject(t *testing.T) {
	repo := domain.Repo{Host: "forge.example", Path: "acme/app"}
	app := domain.Entry{Repo: repo, Ref: "main", Dir: "app", Setup: "make setup"}
	api := domain.Entry{Repo: domain.Repo{Host: "forge.example", Path: "acme/api"}, Ref: "main", Dir: "api"}
	p, err := domain.NewProject("shop", []domain.Entry{app, api})
	if err != nil {
		t.Fatal(err)
	}
	if got := p.Entries(); len(got) != 2 || got[0] != app || got[1] != api || p.Name() != "shop" {
		t.Errorf("project = %v %v", p.Name(), got)
	}
	bad := map[string][]domain.Entry{
		"no entries":   nil,
		"dir twice":    {app, app},
		"absolute dir": {{Repo: repo, Ref: "main", Dir: "/app"}},
		"escaping dir": {{Repo: repo, Ref: "main", Dir: "../app"}},
		"unclean dir":  {{Repo: repo, Ref: "main", Dir: "a//b"}},
		"no repo":      {{Ref: "main", Dir: "app"}},
		"no host":      {{Repo: domain.Repo{Path: "acme/app"}, Ref: "main", Dir: "app"}},
		"no path":      {{Repo: domain.Repo{Host: "forge.example"}, Ref: "main", Dir: "app"}},
		"no ref":       {{Repo: repo, Dir: "app"}},
		"empty dir":    {{Repo: repo, Ref: "main"}},
	}
	for name, entries := range bad {
		if _, err := domain.NewProject("shop", entries); !errors.Is(err, domain.ErrBadProject) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	if _, err := domain.NewProject("", []domain.Entry{app}); !errors.Is(err, domain.ErrBadProject) {
		t.Errorf("no name: err = %v", err)
	}
}
