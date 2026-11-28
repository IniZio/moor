package domain_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

var app = domain.Repo{Host: "forge.example", Path: "acme/app"}

var answerable = map[domain.EventSource]bool{"forge": true}

func evented(spec domain.SpecName, source domain.EventSource) domain.Task {
	return domain.Task{
		Repo: app, Spec: &domain.Spec{Name: spec, File: "specs/" + string(spec) + ".yaml"},
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
	if _, ok := (domain.Task{ID: "t3", Repo: app}).Key(); ok {
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

func TestKeyHoldsTheWholeRepository(t *testing.T) {
	first := evented("review", "forge")
	k1, _ := first.Key()
	for name, mutate := range map[string]func(*domain.Task){
		"other host": func(x *domain.Task) { x.Repo.Host = "other.example" },
		"other path": func(x *domain.Task) { x.Repo.Path = "acme/api" },
	} {
		other := evented("review", "forge")
		mutate(&other)
		if k2, _ := other.Key(); k2 == k1 {
			t.Errorf("%s: shares the key %v", name, k1)
		}
	}
}

func TestRepoComplete(t *testing.T) {
	if !app.Complete() || (domain.Repo{Host: "forge.example"}).Complete() || (domain.Repo{Path: "acme/app"}).Complete() {
		t.Error("Complete must need both the host and the path")
	}
	if got := app.String(); got != "forge.example/acme/app" {
		t.Errorf("String = %q", got)
	}
}
