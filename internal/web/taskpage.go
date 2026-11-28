package web

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// listView is the part of the Tasks screen that the refresh replaces: the count and the rows.
type listView struct {
	Count string
	Rows  []rowView
	Empty string
}

// tasksView is everything the Tasks screen draws. Spec is the selected spec when one is, and SpecName names the
// selection when the team's home has no such spec any more.
type tasksView struct {
	Page     page
	Specs    []specEntry
	Spec     *specEntry
	SpecName string
	Prompt   bool
	Repos    []domain.Repo
	People   []string
	Filter   domain.TaskFilter
	List     listView
	// Stamp is the state of the task table the page was drawn from, which the stream tells changes against.
	Stamp string
}

func (v tasksView) filterBy() string {
	if v.Filter.ByEvent {
		return domain.EventStarterPrefix
	}
	return v.Filter.StartedBy
}

func (v tasksView) filterRepo() string {
	if v.Filter.Repo.Complete() {
		return v.Filter.Repo.String()
	}
	return ""
}

func (v tasksView) peopleSig() string { return peopleSig(v.People) }

func peopleSig(people []string) string {
	sum := sha256.Sum256([]byte(strings.Join(people, "\x00")))
	return hex.EncodeToString(sum[:])[:12]
}

func (s *server) now() time.Time {
	if s.deps.Now != nil {
		return s.deps.Now()
	}
	return time.Now()
}

func (s *server) listOf(r *http.Request, f domain.TaskFilter) (listView, error) {
	list, err := s.deps.Tasks.ListTasks(r.Context(), f)
	if err != nil {
		return listView{}, fmt.Errorf("list tasks: %w", err)
	}
	v := listView{Count: countText(len(list.Items), list.More), Rows: make([]rowView, len(list.Items))}
	for i, it := range list.Items {
		v.Rows[i] = rowOf(it, s.now())
	}
	v.Empty = "No tasks yet."
	if f.Narrowed() {
		v.Empty = "No tasks match these filters."
	}
	return v, nil
}

// selected finds the left-list entry the filter's spec names, by spec name or by file for a file with no spec.
func selected(specs []specEntry, key string) *specEntry {
	for i := range specs {
		if specs[i].Name == key || (specs[i].Name == "" && specs[i].File == key) {
			return &specs[i]
		}
	}
	return nil
}

func (s *server) tasksPage(w http.ResponseWriter, r *http.Request) {
	f, err := parseFilter(r.URL.Query())
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	v, err := s.tasksView(r, f)
	if err != nil {
		serverError(w, r, err)
		return
	}
	render(w, r, http.StatusOK, tasksPage(v))
}

func (s *server) tasksView(r *http.Request, f domain.TaskFilter) (tasksView, error) {
	ctx := r.Context()
	v := tasksView{Page: s.pageData(r), Filter: f, Prompt: f.FromPrompt}
	var err error
	if v.Stamp, err = s.deps.Tasks.TaskStamp(ctx); err != nil {
		return v, fmt.Errorf("task stamp: %w", err)
	}
	if v.Repos, err = s.deps.Settings.Allowed(ctx); err != nil {
		return v, fmt.Errorf("allowed repositories: %w", err)
	}
	if v.Specs, err = s.specEntries(ctx, v.Repos); err != nil {
		return v, err
	}
	if f.Spec != "" {
		if v.Spec = selected(v.Specs, string(f.Spec)); v.Spec == nil {
			v.SpecName = string(f.Spec)
		}
	}
	if v.People, err = s.deps.Tasks.Starters(ctx, scopeOf(f)); err != nil {
		return v, fmt.Errorf("list starters: %w", err)
	}
	v.List, err = s.listOf(r, f)
	return v, err
}

func scopeOf(f domain.TaskFilter) domain.TaskFilter {
	return domain.TaskFilter{Spec: f.Spec, FromPrompt: f.FromPrompt}
}

// listFragment answers the refresh. The rows replace the list, the count rides along, and the Started by menu
// comes only when the people differ from those the page was drawn with, since replacing a menu closes it.
func (s *server) listFragment(w http.ResponseWriter, r *http.Request, f domain.TaskFilter) {
	list, err := s.listOf(r, f)
	if err != nil {
		serverError(w, r, err)
		return
	}
	people, err := s.deps.Tasks.Starters(r.Context(), scopeOf(f))
	if err != nil {
		serverError(w, r, err)
		return
	}
	v := tasksView{Filter: f, People: people, List: list}
	w.Header().Add("Vary", "HX-Request")
	render(w, r, http.StatusOK, listResponse(v, r.URL.Query().Get("people") != v.peopleSig()))
}
