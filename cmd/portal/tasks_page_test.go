package main

import (
	"net/http"
	"net/url"
	"strings"
	"testing"
)

func homeWithSpecs(t *testing.T) func(*config) {
	return withHome(writeHome(t, map[string]string{
		"tasks/pr-evidence.yaml":      prSpec,
		"tasks/dependency-check.yaml": dependencySpec,
		"tasks/broken.yaml":           "name: x\non:\n  push:\nprompt: p\n",
		"tasks/other.yaml":            otherRepoSpec,
	}))
}

func TestTheTasksPageHasTheLeftListTheTaskBoxAndAnEmptyList(t *testing.T) {
	a := newApp(t, homeWithSpecs(t)).withRepos()
	r := a.do(get("Mia", "/tasks"))
	r.has(t, http.StatusOK,
		`<h1>All tasks</h1>`, `What should the agent do?`, `Start task`,
		`<option value="github.com/example-org/shop-web">example-org/shop-web</option>`,
		`aria-label="Task specs"`, `Task specs</h3>`, `PR evidence`, `Dependency check`, `tasks/broken.yaml`, `From a prompt`,
		`href="/tasks?spec=PR+evidence"`, `href="/tasks?from=prompt"`,
		`hx-trigger="change, sse:tasks, task-started from:body"`, `hx-get="/api/tasks"`,
		`id="count"`, `0 tasks`, `No tasks yet.`)
	r.has(t, http.StatusOK, `<a href="/tasks" aria-current><b>All tasks</b></a>`)
	r.lacks(t, `Nothing here yet.`, "src=\"http", "https://")
}

func TestTheTaskBoxExplainsWhenSettingsAllowNoRepository(t *testing.T) {
	a := newApp(t)
	a.do(get("Mia", "/tasks")).has(t, http.StatusOK, "Add a repository under repositories in Settings first.")
}

func TestStartedTasksShowAsRowsWithTheirSecondLine(t *testing.T) {
	a := newApp(t, homeWithSpecs(t)).withRepos()
	a.do(startJSON("Mia", "example-org/shop-web", "Add a coupon <b>field</b>\nsecond line")).has(t, http.StatusCreated)
	a.do(startJSON("Ken", "example-org/shop-web", "Fix links")).has(t, http.StatusCreated)
	r := a.do(get("Mia", "/tasks"))
	r.has(t, http.StatusOK, `2 tasks`, `href="/tasks/1"`, `href="/tasks/2"`,
		`Add a coupon &lt;b&gt;field&lt;/b&gt;`, `#1: started by Mia`, `#2: started by Ken`, `example-org/shop-web`, `just now`, `0s`, `aria-label="queued"`)
	r.lacks(t, `<b>field</b>`, `second line`, `No tasks yet.`)
	if strings.Index(r.body, "Fix links") > strings.Index(r.body, "Add a coupon") {
		t.Error("the newest task is not on top")
	}
	for _, name := range []string{"Mia", "Ken"} {
		r.has(t, http.StatusOK, `<option value="`+name+`">`+name+`</option>`)
	}
	r.has(t, http.StatusOK, `<option value="event:">An event</option>`, `<option value="queued">queued</option>`, `<option value="cancelled">cancelled</option>`)
}

func TestTheListFragmentCarriesRowsTheCountAndAnEmptyState(t *testing.T) {
	a := newApp(t).withRepos()
	a.do(startJSON("Mia", "example-org/shop-web", "One")).has(t, http.StatusCreated)
	a.do(startJSON("Ken", "example-org/shop-web", "Two")).has(t, http.StatusCreated)
	frag := a.do(hx(get("Mia", "/api/tasks?started_by=Ken")))
	frag.has(t, http.StatusOK, `href="/tasks/2"`, `<b id="count" hx-swap-oob="true">1 task</b>`)
	frag.lacks(t, `<html`, `href="/tasks/1"`)
	empty := a.do(hx(get("Mia", "/api/tasks?state=done")))
	empty.has(t, http.StatusOK, "No tasks match these filters.", `0 tasks`)
	if got := empty.header.Get("Vary"); !strings.Contains(got, "HX-Request") {
		t.Errorf("Vary = %q, the same URL answers JSON and a fragment", got)
	}
}

func TestTheStartedByMenuIsOnlyReplacedWhenThePeopleChange(t *testing.T) {
	a := newApp(t).withRepos()
	a.do(startJSON("Mia", "example-org/shop-web", "One")).has(t, http.StatusCreated)
	page := a.do(get("Mia", "/tasks"))
	sig := between(t, page.body, `name="people" value="`, `"`)
	same := a.do(hx(get("Mia", "/api/tasks?people="+url.QueryEscape(sig))))
	same.has(t, http.StatusOK)
	same.lacks(t, `id="startedby"`)
	a.do(startJSON("Ken", "example-org/shop-web", "Two")).has(t, http.StatusCreated)
	changed := a.do(hx(get("Mia", "/api/tasks?started_by=Ken&people="+url.QueryEscape(sig))))
	changed.has(t, http.StatusOK, `id="startedby"`, `hx-swap-oob="true"`, `<option value="Ken" selected>Ken</option>`, `<option value="Mia">Mia</option>`, `id="peoplesig"`)
	if between(t, changed.body, `id="peoplesig"`, `>`) == between(t, page.body, `id="peoplesig"`, `>`) {
		t.Error("the people signature did not change with the people")
	}
}

func between(t *testing.T, s, from, to string) string {
	t.Helper()
	i := strings.Index(s, from)
	if i < 0 {
		t.Fatalf("no %q in %s", from, s)
	}
	rest := s[i+len(from):]
	j := strings.Index(rest, to)
	if j < 0 {
		t.Fatalf("no %q after %q in %s", to, from, s)
	}
	return rest[:j]
}

func TestFilterMenusKeepTheirChoiceInThePageAndTheRefresh(t *testing.T) {
	a := newApp(t).withRepos()
	seedMixed(a)
	page := a.do(get("Mia", "/tasks?state=done&started_by=event:&repository=github.com/example-org/shop-web"))
	page.has(t, http.StatusOK, `<option value="done" selected>done</option>`, `<option value="event:" selected>An event</option>`,
		`<option value="github.com/example-org/shop-web" selected>example-org/shop-web</option>`, `1 task`)
	form := between(t, page.body, `<form id="filters"`, `</form>`)
	for _, want := range []string{`name="spec"`, `name="from"`, `name="people"`, `name="repository"`, `name="started_by"`, `name="state"`} {
		if !strings.Contains(form, want) {
			t.Errorf("the filter form lacks %s", want)
		}
	}
	if strings.Contains(between(t, page.body, `<div id="rows"`, `</main>`), "<select") {
		t.Error("a menu sits inside the part the refresh replaces")
	}
}

func TestASpecShowsItsBannerAndOnlyItsTasks(t *testing.T) {
	a := newApp(t, homeWithSpecs(t)).withRepos()
	seedMixed(a)
	r := a.do(get("Mia", "/tasks?spec=PR+evidence"))
	r.has(t, http.StatusOK, `<h1>PR evidence</h1>`, `tasks/pr-evidence.yaml`,
		`Starts when a pull request is opened or updated on any repository in Settings. You can also start it by hand.`,
		`<button class="btn" type="button" disabled>Run`, `<b>PR evidence</b> #6: github event`, `<b>PR evidence</b> #7: github event, replaced by #9`, `3 tasks`)
	r.lacks(t, `What should the agent do?`, `#1: started by Mia`, `Dependency check</b>`)
	d := a.do(get("Mia", "/tasks?spec=Dependency+check"))
	d.has(t, http.StatusOK, `on the schedule 0 6 * * 1, one task per repository.`, `#8: Run by Ken`, `1 task`)
	d.lacks(t, `disabled>Run`, `You can also start it by hand.`)
}

func TestABrokenSpecShowsItsProblemInsteadOfAList(t *testing.T) {
	a := newApp(t, homeWithSpecs(t)).withRepos()
	b := a.do(get("Mia", "/tasks?spec=tasks/broken.yaml"))
	b.has(t, http.StatusOK, `tasks/broken.yaml`, `unknown key`, `role="alert"`)
	b.lacks(t, `id="rows"`, `Start task`)
	o := a.do(get("Mia", "/tasks?spec=Other+repo"))
	o.has(t, http.StatusOK, `<h1>Other repo</h1>`, `example-org/other is not under repositories in Settings. Add it there first.`, `id="rows"`)
	gone := a.do(get("Mia", "/tasks?spec=Old+spec"))
	gone.has(t, http.StatusOK, `<h1>Old spec</h1>`, `is not in the team's home`, `No tasks yet.`)
}

func TestFromAPromptListsOnlyPersonStartedTasks(t *testing.T) {
	a := newApp(t, homeWithSpecs(t)).withRepos()
	seedMixed(a)
	r := a.do(get("Mia", "/tasks?from=prompt"))
	r.has(t, http.StatusOK, `<h1>From a prompt</h1>`, `Tasks people started by typing a prompt.`, `5 tasks`, `#1: started by Mia`)
	r.lacks(t, `What should the agent do?`, `github event`)
	a.do(get("Mia", "/tasks?from=nope")).has(t, http.StatusBadRequest)
}

func TestStartingFromTheBoxAnswersWithToastsAndAFreshBox(t *testing.T) {
	a := newApp(t).withRepos()
	ok := a.do(hx(startForm("Mia", "github.com/example-org/shop-web", "Add a coupon")))
	ok.has(t, http.StatusOK, "Started task #1 on example-org/shop-web.", `id="toast"`, `id="newtask"`, `hx-swap-oob="true"`)
	if ok.header.Get("HX-Trigger") != "task-started" {
		t.Errorf("HX-Trigger = %q, the list must refresh at once", ok.header.Get("HX-Trigger"))
	}
	ok.lacks(t, "Add a coupon")
	for _, c := range []struct{ repo, prompt, toast string }{
		{"github.com/example-org/shop-web", "  ", "Write what the agent should do first."},
		{"github.com/example-org/other", "go", "example-org/other is not under repositories in Settings. Add it there first."},
	} {
		bad := a.do(hx(startForm("Mia", c.repo, c.prompt)))
		bad.has(t, http.StatusUnprocessableEntity, c.toast, `id="toast"`)
		bad.lacks(t, `id="newtask"`)
		if bad.header.Get("HX-Reswap") != "none" || bad.header.Get("HX-Trigger") != "" {
			t.Errorf("a refused start must leave the page as it is: %v", bad.header)
		}
	}
	if got := a.list("Mia", ""); len(got.Tasks) != 1 {
		t.Errorf("%d tasks, want 1", len(got.Tasks))
	}
}

func TestARunPageStandsInForEveryTask(t *testing.T) {
	a := newApp(t).withRepos()
	seedMixed(a)
	p := a.do(get("Mia", "/tasks/1"))
	p.has(t, http.StatusOK, "Prompt of 1", "#1", "example-org/shop-web", `href="/tasks?from=prompt"`, "From a prompt", `aria-label="queued"`)
	s := a.do(get("Mia", "/tasks/6"))
	s.has(t, http.StatusOK, "PR evidence", `href="/tasks?spec=PR+evidence"`)
	a.do(get("Mia", "/tasks/999")).has(t, http.StatusNotFound)
	a.do(get("", "/tasks/1")).has(t, http.StatusUnauthorized)
}
