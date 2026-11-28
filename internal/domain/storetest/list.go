package storetest

import (
	"context"
	"fmt"
	"slices"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// ListStore is a task store that also lists tasks for the portal and hands out task numbers.
type ListStore interface {
	Store
	ListTasks(ctx context.Context, f domain.TaskFilter) (domain.TaskList, error)
	// Starters lists the people who started tasks in the scope of f, by Spec and FromPrompt only.
	Starters(ctx context.Context, f domain.TaskFilter) ([]string, error)
	NextTaskID(ctx context.Context) (domain.TaskID, error)
}

// RunList runs the listing cases on a fresh store each.
func RunList(t *testing.T, fresh func(t *testing.T) ListStore) {
	t.Helper()
	cases := []struct {
		name string
		run  func(*testing.T, ListStore)
	}{
		{"newest first, a tie broken by the number", listOrder},
		{"each filter", listFilters},
		{"filters together", listCombined},
		{"a limit says there are more", listLimit},
		{"a superseded task names its replacement", listReplacedBy},
		{"starters are the people in the scope", listStarters},
		{"task numbers go up and are never repeated", listNumbers},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) { c.run(t, fresh(t)) })
	}
}

type seed struct {
	id    domain.TaskID
	age   int64
	state domain.State
	by    string
	spec  string
	repo  string
	event bool
}

func (sd seed) task() domain.Task {
	repo := shop
	if sd.repo != "" {
		repo.Path = domain.RepoPath(sd.repo)
	}
	t := domain.Task{ID: sd.id, Repo: repo, Prompt: "p", StartedBy: sd.by, State: sd.state, LastSeq: 1, CreatedAt: at(sd.age)}
	if sd.spec != "" {
		t.Spec = &domain.Spec{Name: domain.SpecName(sd.spec), File: "tasks/" + sd.spec + ".yaml"}
	}
	if sd.event {
		t.Event = &domain.Event{Source: "forge", Group: "g", Rev: domain.Revision("r" + string(sd.id))}
	}
	return t
}

func seedAll(t *testing.T, s ListStore, seeds ...seed) {
	t.Helper()
	for _, sd := range seeds {
		create(t, Subject{Store: s}, sd.task())
	}
}

func listed(t *testing.T, s ListStore, f domain.TaskFilter) []string {
	t.Helper()
	got, err := s.ListTasks(context.Background(), f)
	if err != nil {
		t.Fatalf("list %+v: %v", f, err)
	}
	ids := make([]string, len(got.Items))
	for i, it := range got.Items {
		ids[i] = string(it.Task.ID)
	}
	return ids
}

func wantIDs(t *testing.T, what string, got []string, want ...string) {
	t.Helper()
	if !slices.Equal(got, want) {
		t.Errorf("%s = %v, want %v", what, got, want)
	}
}

func listOrder(t *testing.T, s ListStore) {
	seedAll(t, s,
		seed{id: "9", age: 5, state: domain.Done, by: "Mia"},
		seed{id: "10", age: 5, state: domain.Done, by: "Mia"},
		seed{id: "2", age: 1, state: domain.Done, by: "Mia"},
		seed{id: "11", age: 9, state: domain.Waiting, by: "Ken"},
	)
	wantIDs(t, "all", listed(t, s, domain.TaskFilter{}), "11", "10", "9", "2")
}

func listFilters(t *testing.T, s ListStore) {
	seedAll(t, s,
		seed{id: "1", age: 1, state: domain.Waiting, by: "Mia"},
		seed{id: "2", age: 2, state: domain.Starting, by: "Ken", repo: "docs"},
		seed{id: "3", age: 3, state: domain.Running, by: "Mia"},
		seed{id: "4", age: 4, state: domain.Delivering, by: "Mia", spec: "PR evidence", event: true},
		seed{id: "5", age: 5, state: domain.Done, by: domain.EventStarterPrefix + "forge", spec: "PR evidence", event: true},
		seed{id: "6", age: 6, state: domain.Failed, by: "Ken", spec: "Dependency check", repo: "docs"},
		seed{id: "7", age: 7, state: domain.Cancelled, by: "Mia"},
		seed{id: "8", age: 8, state: domain.Superseded, by: domain.EventStarterPrefix + "forge", spec: "Dependency check", event: true},
	)
	for name, tc := range map[string]struct {
		f    domain.TaskFilter
		want []string
	}{
		"a spec":                   {domain.TaskFilter{Spec: "PR evidence"}, []string{"5", "4"}},
		"a spec nobody ran":        {domain.TaskFilter{Spec: "Nope"}, nil},
		"from a prompt":            {domain.TaskFilter{FromPrompt: true}, []string{"7", "3", "2", "1"}},
		"a repository":             {domain.TaskFilter{Repo: domain.Repo{Host: shop.Host, Path: "docs"}}, []string{"6", "2"}},
		"a person":                 {domain.TaskFilter{StartedBy: "Ken"}, []string{"6", "2"}},
		"a person's name is exact": {domain.TaskFilter{StartedBy: "ken"}, nil},
		"an event":                 {domain.TaskFilter{ByEvent: true}, []string{"8", "5"}},
		"queued":                   {domain.TaskFilter{Group: domain.GroupQueued}, []string{"1"}},
		"running holds three":      {domain.TaskFilter{Group: domain.GroupRunning}, []string{"4", "3", "2"}},
		"done":                     {domain.TaskFilter{Group: domain.GroupDone}, []string{"5"}},
		"failed":                   {domain.TaskFilter{Group: domain.GroupFailed}, []string{"6"}},
		"cancelled holds two":      {domain.TaskFilter{Group: domain.GroupCancelled}, []string{"8", "7"}},
	} {
		wantIDs(t, name, listed(t, s, tc.f), tc.want...)
	}
}

func listCombined(t *testing.T, s ListStore) {
	seedAll(t, s,
		seed{id: "1", age: 1, state: domain.Done, by: "Mia"},
		seed{id: "2", age: 2, state: domain.Failed, by: "Mia"},
		seed{id: "3", age: 3, state: domain.Done, by: "Ken"},
		seed{id: "4", age: 4, state: domain.Done, by: "Mia", repo: "docs"},
	)
	f := domain.TaskFilter{StartedBy: "Mia", Group: domain.GroupDone, Repo: shop}
	wantIDs(t, "Mia, done, shop", listed(t, s, f), "1")
	f.Repo = domain.Repo{}
	wantIDs(t, "Mia, done", listed(t, s, f), "4", "1")
	f.FromPrompt = true
	f.Spec = "PR evidence"
	wantIDs(t, "from a prompt and a spec", listed(t, s, f))
}

func listLimit(t *testing.T, s ListStore) {
	for i := range 5 {
		seedAll(t, s, seed{id: domain.TaskID(fmt.Sprint(i + 1)), age: int64(i), state: domain.Done, by: "Mia"})
	}
	got, err := s.ListTasks(context.Background(), domain.TaskFilter{Limit: 3})
	if err != nil || len(got.Items) != 3 || !got.More {
		t.Fatalf("limit 3 gave %d rows, more %v, %v", len(got.Items), got.More, err)
	}
	got, err = s.ListTasks(context.Background(), domain.TaskFilter{Limit: 5})
	if err != nil || len(got.Items) != 5 || got.More {
		t.Fatalf("limit 5 gave %d rows, more %v, %v", len(got.Items), got.More, err)
	}
}

func listReplacedBy(t *testing.T, s ListStore) {
	older, newer := seed{id: "1", age: 1, state: domain.Superseded, spec: "PR evidence", event: true}, seed{id: "2", age: 2, state: domain.Running, spec: "PR evidence", event: true}
	seedAll(t, s, older, newer, seed{id: "3", age: 3, state: domain.Superseded, by: "Mia"})
	got, err := s.ListTasks(context.Background(), domain.TaskFilter{})
	if err != nil {
		t.Fatal(err)
	}
	byID := map[domain.TaskID]domain.TaskID{}
	for _, it := range got.Items {
		byID[it.Task.ID] = it.ReplacedBy
	}
	if byID["1"] != "2" || byID["2"] != "" || byID["3"] != "" {
		t.Errorf("replaced by = %v, want task 1 replaced by 2 and nothing else", byID)
	}
}

func listStarters(t *testing.T, s ListStore) {
	seedAll(t, s,
		seed{id: "1", age: 1, state: domain.Done, by: "Mia"},
		seed{id: "2", age: 2, state: domain.Done, by: "ken"},
		seed{id: "3", age: 3, state: domain.Done, by: "Mia", spec: "PR evidence", event: true},
		seed{id: "4", age: 4, state: domain.Done, by: domain.EventStarterPrefix + "forge", spec: "PR evidence", event: true},
		seed{id: "5", age: 5, state: domain.Done, by: "Ana", spec: "Dependency check"},
	)
	ctx := context.Background()
	for name, tc := range map[string]struct {
		f    domain.TaskFilter
		want []string
	}{
		"all":                       {domain.TaskFilter{}, []string{"Ana", "ken", "Mia"}},
		"a spec":                    {domain.TaskFilter{Spec: "PR evidence"}, []string{"Mia"}},
		"from prompt":               {domain.TaskFilter{FromPrompt: true}, []string{"ken", "Mia"}},
		"ignores the other filters": {domain.TaskFilter{StartedBy: "Ana", Group: domain.GroupFailed}, []string{"Ana", "ken", "Mia"}},
	} {
		got, err := s.Starters(ctx, tc.f)
		if err != nil || !slices.Equal(got, tc.want) {
			t.Errorf("%s: %v, %v, want %v", name, got, err, tc.want)
		}
	}
}

func listNumbers(t *testing.T, s ListStore) {
	seen := map[domain.TaskID]bool{}
	var last domain.TaskID
	for range 5 {
		id, err := s.NextTaskID(context.Background())
		if err != nil || id == "" || seen[id] {
			t.Fatalf("number %q, %v, seen before: %v", id, err, seen[id])
		}
		if len(id) < len(last) || (len(id) == len(last) && id <= last) {
			t.Errorf("number %q does not follow %q", id, last)
		}
		seen[id], last = true, id
	}
}
