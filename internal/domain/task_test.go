package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestTaskKey(t *testing.T) {
	trigger := &domain.Trigger{Source: "forge", Group: "acme/app#7", Rev: "abc", Context: []byte(`{"n":7}`)}
	first := domain.Task{ID: "t1", Project: "app", Trigger: trigger}
	redelivered := domain.Task{ID: "t2", Project: "app", Trigger: trigger}
	k1, ok1 := first.Key()
	k2, ok2 := redelivered.Key()
	if !ok1 || !ok2 || k1 != k2 {
		t.Fatalf("keys %v %v, %v %v", k1, ok1, k2, ok2)
	}
	if first.Flow() != domain.Triggered {
		t.Errorf("flow = %s", first.Flow())
	}
	free := domain.Task{ID: "t3", Project: "app"}
	if _, ok := free.Key(); ok || free.Flow() != domain.FreePrompt {
		t.Errorf("free-prompt task has a key or flow %s", free.Flow())
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
