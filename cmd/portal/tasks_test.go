package main

import (
	"context"
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
)

var shopWeb = domain.Repo{Host: "github.com", Path: "example-org/shop-web"}

type listed struct {
	Tasks []struct {
		ID         string `json:"id"`
		Title      string `json:"title"`
		State      string `json:"state"`
		Group      string `json:"group"`
		Spec       string `json:"spec"`
		StartedBy  string `json:"started_by"`
		ReplacedBy string `json:"replaced_by"`
		Repository struct{ Host, Path string }
	} `json:"tasks"`
	More bool `json:"more"`
}

func (a *app) list(person, query string) listed {
	a.t.Helper()
	r := a.do(get(person, "/api/tasks"+query))
	r.has(a.t, http.StatusOK)
	var out listed
	if err := json.Unmarshal([]byte(r.body), &out); err != nil {
		a.t.Fatalf("list %q: %v in %s", query, err, r.body)
	}
	return out
}

func (a *app) ids(query string) string {
	a.t.Helper()
	var ids []string
	for _, t := range a.list("Mia", query).Tasks {
		ids = append(ids, t.ID)
	}
	return strings.Join(ids, " ")
}

func startJSON(person, repo, prompt string) req {
	b, _ := json.Marshal(map[string]string{"repository": repo, "prompt": prompt})
	return req{method: http.MethodPost, path: "/api/tasks", person: person, json: true, body: string(b)}
}

func startForm(person, repo, prompt string) req {
	return req{method: http.MethodPost, path: "/api/tasks", person: person, form: url.Values{"repository": {repo}, "prompt": {prompt}}}
}

// withRepos saves the settings document that allows shopWeb.
func (a *app) withRepos() *app {
	a.t.Helper()
	a.do(put("Mia", 0, goodDoc)).has(a.t, http.StatusOK)
	return a
}

// seed stores tasks that nothing in this test started, as the runner and the webhook will.
func (a *app) seed(tasks ...domain.Task) {
	a.t.Helper()
	ctx := context.Background()
	db, err := store.Open(ctx, a.dsn)
	if err != nil {
		a.t.Fatal(err)
	}
	defer db.Close()
	for _, t := range tasks {
		t.LastSeq = 1
		ev := domain.EventRow{Task: t.ID, Seq: 1, At: t.CreatedAt, Kind: domain.KindNote, Data: json.RawMessage(`{"text":"seeded"}`)}
		if _, err := db.Save(ctx, domain.TaskWrite{Task: t, Create: true, Events: []domain.EventRow{ev}}); err != nil {
			a.t.Fatalf("seed %s: %v", t.ID, err)
		}
	}
}

func seeded(id string, age int, state domain.State, by string) domain.Task {
	return domain.Task{
		ID: domain.TaskID(id), Repo: shopWeb, Prompt: "Prompt of " + id + "\nsecond line", StartedBy: by, State: state,
		CreatedAt: time.Date(2026, 11, 20, 12, 0, age, 0, time.UTC),
	}
}

func fromSpec(t domain.Task, spec string) domain.Task {
	t.Spec = &domain.Spec{Name: domain.SpecName(spec), File: "tasks/" + spec + ".yaml"}
	t.Event = &domain.Event{Source: "github", Group: domain.EventGroup("pr-" + string(t.ID)), Rev: "rev" + domain.Revision(t.ID)}
	t.StartedBy = domain.EventStarterPrefix + "github"
	return t
}

func TestAPersonStartsATaskAndItTopsTheList(t *testing.T) {
	a := newApp(t).withRepos()
	first := a.do(startJSON("Mia", "example-org/shop-web", "Add a coupon field\nto checkout"))
	first.has(t, http.StatusCreated, `"id":"1"`, `"title":"Add a coupon field"`, `"state":"waiting"`, `"group":"queued"`, `"started_by":"Mia"`, `"path":"example-org/shop-web"`)
	a.do(startForm("Ken", "github.com/example-org/shop-web", "  Fix the links  ")).has(t, http.StatusCreated, `"id":"2"`, `"title":"Fix the links"`)
	got := a.list("Mia", "")
	if len(got.Tasks) != 2 || got.Tasks[0].ID != "2" || got.Tasks[1].ID != "1" || got.More {
		t.Fatalf("list = %+v", got)
	}
	if got.Tasks[1].Repository.Host != "github.com" {
		t.Errorf("repository = %+v", got.Tasks[1].Repository)
	}
}

func TestATaskIsRefusedWhenThePromptOrTheRepositoryIsNotGood(t *testing.T) {
	a := newApp(t)
	a.do(startJSON("Mia", "example-org/shop-web", "go")).has(t, http.StatusUnprocessableEntity, "example-org/shop-web is not under repositories in Settings. Add it there first.")
	a.withRepos()
	a.do(startJSON("Mia", "example-org/shop-web", " \n ")).has(t, http.StatusUnprocessableEntity, "Write what the agent should do first.")
	a.do(startJSON("Mia", "example-org/other", "go")).has(t, http.StatusUnprocessableEntity, "example-org/other is not under repositories in Settings. Add it there first.")
	a.do(startJSON("Mia", "gitlab.example.com/example-org/shop-web", "go")).has(t, http.StatusUnprocessableEntity, "example-org/shop-web is not under repositories in Settings.")
	a.do(startForm("Mia", "not a repo", "go")).has(t, http.StatusUnprocessableEntity, "is not a repository")
	a.do(startForm("Mia", "", "go")).has(t, http.StatusUnprocessableEntity, "is not a repository")
	a.do(req{method: http.MethodPost, path: "/api/tasks", person: "Mia", json: true, body: "{"}).has(t, http.StatusBadRequest)
	if got := a.list("Mia", ""); len(got.Tasks) != 0 {
		t.Errorf("a refused start made %d tasks", len(got.Tasks))
	}
}

func TestTheTaskRoutesNeedAPersonAndRefuseACrossSitePost(t *testing.T) {
	a := newApp(t).withRepos()
	a.do(get("", "/api/tasks")).has(t, http.StatusUnauthorized)
	a.do(startJSON("", "example-org/shop-web", "go")).has(t, http.StatusUnauthorized)
	for _, headers := range []map[string]string{{"Sec-Fetch-Site": "cross-site"}, {"Origin": "https://evil.example"}} {
		r := startJSON("Mia", "example-org/shop-web", "go")
		r.headers = headers
		a.do(r).has(t, http.StatusForbidden)
	}
	if got := a.list("Mia", ""); len(got.Tasks) != 0 {
		t.Errorf("a cross-site start made %d tasks", len(got.Tasks))
	}
}

func seedMixed(a *app) {
	docs := domain.Repo{Host: "github.com", Path: "example-org/docs-site"}
	t3 := seeded("3", 3, domain.Running, "Ken")
	t3.Repo = docs
	t6 := fromSpec(seeded("6", 6, domain.Done, ""), "PR evidence")
	t7 := fromSpec(seeded("7", 7, domain.Superseded, ""), "PR evidence")
	t9 := fromSpec(seeded("9", 9, domain.Running, ""), "PR evidence")
	t9.Event.Group = t7.Event.Group
	t8 := fromSpec(seeded("8", 8, domain.Failed, "Ken"), "Dependency check")
	t8.StartedBy = "Ken"
	a.seed(seeded("1", 1, domain.Waiting, "Mia"), seeded("2", 2, domain.Done, "Mia"), t3,
		seeded("4", 4, domain.Failed, "Ana"), seeded("5", 5, domain.Cancelled, "Mia"), t6, t7, t8, t9)
}

func TestTheListNarrowsByEachFilter(t *testing.T) {
	a := newApp(t)
	seedMixed(a)
	for query, want := range map[string]string{
		"":                                  "9 8 7 6 5 4 3 2 1",
		"?spec=PR+evidence":                 "9 7 6",
		"?spec=Dependency+check":            "8",
		"?spec=Nope":                        "",
		"?from=prompt":                      "5 4 3 2 1",
		"?repository=example-org/docs-site": "3",
		"?repository=github.com/example-org/shop-web": "9 8 7 6 5 4 2 1",
		"?started_by=Mia":    "5 2 1",
		"?started_by=Ken":    "8 3",
		"?started_by=event:": "9 7 6",
		"?state=queued":      "1",
		"?state=running":     "9 3",
		"?state=done":        "6 2",
		"?state=failed":      "8 4",
		"?state=cancelled":   "7 5",
		"?spec=&from=&repository=&started_by=&state=": "9 8 7 6 5 4 3 2 1",
		"?from=prompt&started_by=Mia&state=done":      "2",
		"?spec=PR+evidence&state=cancelled":           "7",
	} {
		if got := a.ids(query); got != want {
			t.Errorf("%s gave %q, want %q", query, got, want)
		}
	}
}

func TestEachRowHasATitleAndSaysWhatReplacedASupersededTask(t *testing.T) {
	a := newApp(t)
	seedMixed(a)
	byID := map[string]struct{ title, spec, group, by, replaced string }{}
	for _, row := range a.list("Mia", "").Tasks {
		byID[row.ID] = struct{ title, spec, group, by, replaced string }{row.Title, row.Spec, row.Group, row.StartedBy, row.ReplacedBy}
	}
	if got := byID["1"]; got.title != "Prompt of 1" || got.spec != "" || got.group != "queued" || got.by != "Mia" {
		t.Errorf("a prompt task = %+v", got)
	}
	if got := byID["6"]; got.title != "PR evidence" || got.spec != "PR evidence" || got.by != "event:github" {
		t.Errorf("an event task = %+v", got)
	}
	if got := byID["7"]; got.group != "cancelled" || got.replaced != "9" {
		t.Errorf("a superseded task = %+v, want it replaced by 9", got)
	}
	if got := byID["5"]; got.group != "cancelled" || got.replaced != "" {
		t.Errorf("a cancelled task = %+v", got)
	}
}

func TestABadFilterIsRefused(t *testing.T) {
	a := newApp(t)
	for _, q := range []string{"?state=waiting", "?state=nope", "?from=person", "?repository=not+a+repo", "?repository=a/b/c"} {
		a.do(get("Mia", "/api/tasks"+q)).has(t, http.StatusBadRequest)
	}
}
