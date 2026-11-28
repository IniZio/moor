package main

import (
	"context"
	"net/http"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

var (
	menuItem   = regexp.MustCompile(`<button[^>]*role="menuitem"[^>]*>([^<]*)</button>`)
	joinToken  = regexp.MustCompile(`-token (\S+) -key`)
	expiryText = regexp.MustCompile(`Run it on the machine\. Shown once, works once, expires \d\d:\d\d\.`)
)

// rowOf cuts one machine's row out of a page or a fragment.
func rowOf(t *testing.T, body, id string) string {
	t.Helper()
	start := strings.Index(body, `id="`+id+`"`)
	if start < 0 {
		t.Fatalf("no row for %s in %s", id, body)
	}
	rest := body[start:]
	if end := strings.Index(rest[1:], `class="lrow mrow"`); end >= 0 {
		rest = rest[:end+1]
	}
	return rest
}

// join runs the join command's request for the machine, as the runner would.
func (a *app) join(id, token, name string) {
	a.t.Helper()
	body, _ := joinBody(a.t, id, token, name)
	a.do(joinReq(body)).has(a.t, http.StatusOK)
}

func (a *app) act(person, id, action string) reply {
	a.t.Helper()
	if action == "remove" {
		return a.do(hx(req{method: http.MethodDelete, path: "/api/machines/" + id, person: person}))
	}
	return a.do(hx(req{method: http.MethodPost, path: "/api/machines/" + id + "/" + action, person: person}))
}

func TestTheMachinesPageShowsAnEmptyStateThenRowsAndOnlyTheEnrollerGetsAMenu(t *testing.T) {
	a := newApp(t, withPublicURL)
	r := a.do(get("Mia", "/machines"))
	r.has(t, http.StatusOK, "<h1>Machines</h1>", "Add machine", "No machines yet.", `hx-ext="sse"`, `sse-connect="/api/machines/events?since=`)
	r.lacks(t, "Nothing here yet.", `src="http`, "https://")
	mine, token := a.addMachine("Mia")
	a.join(mine, token, "host-a")
	theirs, token := a.addMachine("Ken")
	a.join(theirs, token, "host-b")
	page := a.do(get("Mia", "/machines"))
	page.has(t, http.StatusOK, "Enrolled by", "Tasks", "State")
	page.lacks(t, "No machines yet.")
	mineRow, theirRow := rowOf(t, page.body, mine), rowOf(t, page.body, theirs)
	for _, want := range []string{"host-a", "offline", "Mia", `aria-label="Actions for host-a"`} {
		if !strings.Contains(mineRow, want) {
			t.Errorf("Mia's own row lacks %q: %s", want, mineRow)
		}
	}
	for _, want := range []string{"host-b", "offline", "Ken"} {
		if !strings.Contains(theirRow, want) {
			t.Errorf("Ken's row lacks %q as Mia sees it: %s", want, theirRow)
		}
	}
	if strings.Contains(theirRow, "Actions for") || strings.Contains(theirRow, "menuitem") {
		t.Errorf("a member who did not enrol the machine sees a menu: %s", theirRow)
	}
	if !strings.Contains(rowOf(t, a.do(get("Ken", "/machines")).body, theirs), `aria-label="Actions for host-b"`) {
		t.Error("the enroller does not see the menu on a refresh as Ken")
	}
}

func TestTheMenuItemsFollowTheStateAndRemoveComesOnlyAfterARevoke(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	waiting, _ := p.addMachine("Mia")
	offline, token := p.addMachine("Mia")
	p.join(offline, token, "host-off")
	draining, token := p.addMachine("Mia")
	p.join(draining, token, "host-drain")
	p.act("Mia", draining, "drain").has(t, http.StatusOK)
	revoked, token := p.addMachine("Mia")
	p.join(revoked, token, "host-gone")
	p.act("Mia", revoked, "revoke").has(t, http.StatusOK)
	m := enrol(t, p, "host-on")
	m.run(t, p.addr)
	eventually(t, "the machine linked", func() bool { return p.machines("Mia")[m.id].State == "linked" })
	page := p.do(get("Mia", "/machines"))
	for _, c := range []struct {
		id, state string
		items     []string
	}{
		{waiting, "joining", []string{"Revoke"}},
		{offline, "offline", []string{"Revoke"}},
		{draining, "draining", []string{"Resume", "Disconnect", "Revoke"}},
		{revoked, "revoked", []string{"Remove"}},
		{m.id, "online", []string{"Drain", "Disconnect", "Revoke"}},
	} {
		row := rowOf(t, page.body, c.id)
		if got := menuOf(row); !slices.Equal(got, c.items) {
			t.Errorf("row %s offers %v, want %v", c.id, got, c.items)
		}
		if !strings.Contains(row, `<span class="st `+c.state+`">`+c.state+"</span>") {
			t.Errorf("row %s does not show %s: %s", c.id, c.state, row)
		}
	}
}

func TestADrainingRowOffersResumeThatPostsToTheResumeRoute(t *testing.T) {
	a := newApp(t, withPublicURL)
	id, token := a.addMachine("Mia")
	a.join(id, token, "build-1")
	if row := rowOf(t, a.do(get("Mia", "/machines")).body, id); strings.Contains(row, "Resume") {
		t.Errorf("a machine in service offers Resume: %s", row)
	}
	a.act("Mia", id, "drain").has(t, http.StatusOK)
	row := rowOf(t, a.do(get("Mia", "/machines")).body, id)
	if !strings.Contains(row, `hx-post="/api/machines/`+id+`/resume"`) {
		t.Errorf("the draining row's Resume does not post to its route: %s", row)
	}
	a.act("Mia", id, "resume").has(t, http.StatusOK)
	if row := rowOf(t, a.do(hx(get("Mia", "/api/machines"))).body, id); strings.Contains(row, "Resume") {
		t.Errorf("a resumed machine still offers Resume: %s", row)
	}
}

func menuOf(row string) []string {
	var items []string
	for _, it := range menuItem.FindAllStringSubmatch(row, -1) {
		items = append(items, it[1])
	}
	return items
}

func TestAddMachineShowsTheJoinCommandOnceWithACopyControl(t *testing.T) {
	a := newApp(t, withPublicURL)
	add := a.do(hx(req{method: http.MethodPost, path: "/api/machines", person: "Mia"}))
	add.has(t, http.StatusCreated, `id="joincmd"`, "runner join -portal https://portal.example.com -id m-", "Copy", "Done", `data-copy="joincmd"`)
	if !expiryText.MatchString(add.body) {
		t.Errorf("the box lacks the one line of words under the command: %s", add.body)
	}
	if add.header.Get("HX-Trigger") != "machines-changed" {
		t.Errorf("HX-Trigger = %q, the rows must refresh at once", add.header.Get("HX-Trigger"))
	}
	m := joinToken.FindStringSubmatch(add.body)
	if m == nil {
		t.Fatalf("no token in the join command: %s", add.body)
	}
	for name, r := range map[string]reply{
		"the page":      a.do(get("Mia", "/machines")),
		"the rows":      a.do(hx(get("Mia", "/api/machines"))),
		"the JSON list": a.do(get("Mia", "/api/machines")),
	} {
		r.has(t, http.StatusOK)
		r.lacks(t, m[1], "runner join")
		if name == "the page" || name == "the rows" {
			r.has(t, http.StatusOK, "joining")
		}
	}
	again := a.do(hx(req{method: http.MethodPost, path: "/api/machines", person: "Mia"}))
	if other := joinToken.FindStringSubmatch(again.body); other == nil || other[1] == m[1] {
		t.Errorf("a second Add machine must give a new token: %v", other)
	}
}

func TestRowActionsAnswerWithTheToastsOfThePortalPage(t *testing.T) {
	a := newApp(t, withPublicURL).withRepos()
	id, token := a.addMachine("Mia")
	a.join(id, token, "build-1")
	check := func(r reply, want string) {
		t.Helper()
		r.has(t, http.StatusOK, want, `id="toast"`)
		if r.header.Get("HX-Reswap") != "none" || r.header.Get("HX-Trigger") != "machines-changed" {
			t.Errorf("an action must leave the page as it is and refresh the rows: %v", r.header)
		}
	}
	check(a.act("Mia", id, "drain"), "build-1 is draining. Its running tasks finish. It takes no new ones.")
	check(a.act("Mia", id, "disconnect"), "Link to build-1 closed. It may reconnect at once. Its tasks keep running.")
	svc, _ := a.tasks()
	for range 2 {
		running := a.startTask("work")
		if _, err := svc.Report(context.Background(), running, task.Report{Kind: task.SlotTaken, Runner: domain.RunnerID(id)}); err != nil {
			t.Fatal(err)
		}
	}
	check(a.act("Mia", id, "revoke"), "build-1 revoked. Its 2 running tasks failed with the reason &#34;machine revoked&#34;.")
	check(a.act("Mia", id, "remove"), "build-1 removed.")
	if _, ok := a.machines("Mia")[id]; ok {
		t.Error("a removed machine is still listed")
	}
	lone, token := a.addMachine("Mia")
	a.join(lone, token, "build-2")
	check(a.act("Mia", lone, "revoke"), "build-2 revoked.")
}

func TestAnotherMemberIsRefusedEveryActionAndEveryAsk(t *testing.T) {
	a := newApp(t, withPublicURL)
	id, token := a.addMachine("Mia")
	a.join(id, token, "build-1")
	const msg = "Only the member who added this machine can change it."
	for _, action := range []string{"drain", "disconnect", "revoke", "remove"} {
		r := a.act("Ken", id, action)
		r.has(t, http.StatusForbidden, msg, `id="toast"`)
		if r.header.Get("HX-Trigger") != "" {
			t.Errorf("a refused %s must not refresh the rows", action)
		}
	}
	a.do(hx(get("Ken", "/api/machines/"+id+"/ask?verb=revoke"))).has(t, http.StatusForbidden, msg)
	a.do(hx(get("Ken", "/api/machines/"+id+"/menu"))).has(t, http.StatusForbidden, msg)
	if got := a.machines("Mia")[id]; got.State != "offline" {
		t.Errorf("a refused member changed the machine: %+v", got)
	}
}

func TestRevokeAndRemoveAskInTheRowBeforeTheyAct(t *testing.T) {
	a := newApp(t, withPublicURL)
	id, token := a.addMachine("Mia")
	a.join(id, token, "build-1")
	ask := a.do(hx(get("Mia", "/api/machines/"+id+"/ask?verb=revoke")))
	ask.has(t, http.StatusOK, "Revoke build-1?", `hx-post="/api/machines/`+id+`/revoke"`, "Keep", `hx-get="/api/machines/`+id+`/menu"`, `id="act-`+id+`"`)
	ask.lacks(t, "menuitem")
	a.do(hx(get("Mia", "/api/machines/"+id+"/ask?verb=remove"))).has(t, http.StatusConflict, "Revoke the machine before you remove it.")
	a.do(hx(get("Mia", "/api/machines/"+id+"/ask?verb=drain"))).has(t, http.StatusNotFound)
	a.do(hx(get("Mia", "/api/machines/nope/ask?verb=revoke"))).has(t, http.StatusNotFound)
	a.act("Mia", id, "revoke").has(t, http.StatusOK)
	rm := a.do(hx(get("Mia", "/api/machines/"+id+"/ask?verb=remove")))
	rm.has(t, http.StatusOK, "Remove build-1?", `hx-delete="/api/machines/`+id+`"`, "Keep")
	menu := a.do(hx(get("Mia", "/api/machines/"+id+"/menu")))
	menu.has(t, http.StatusOK, `id="act-`+id+`"`, "Remove")
	menu.lacks(t, "Revoke build-1?")
}

func TestTheMachineStreamTellsOfAChangeOnAnotherPortalAndOfNothingElse(t *testing.T) {
	a := newApp(t, withPublicURL)
	b := a.peer()
	page := a.do(get("Mia", "/machines"))
	stamp := between(t, page.body, `sse-connect="/api/machines/events?since=`, `"`)
	page.has(t, http.StatusOK, `hx-trigger="sse:machines[`, `machines-changed from:body"`, `hx-get="/api/machines"`)
	changes := streamOf(t, b.handler, "/api/machines/events?since="+stamp, nil)
	quiet(t, changes, 1500*time.Millisecond)
	a.addMachine("Mia")
	f := next(t, changes)
	if f.typ != "machines" || f.data == "" || f.data == stamp {
		t.Errorf("machines event = %+v, stamp was %q", f, stamp)
	}
	stale := streamOf(t, b.handler, "/api/machines/events?since=0.0", nil)
	if f := next(t, stale); f.typ != "machines" {
		t.Errorf("a stream opened with an old stamp must tell at once, got %+v", f)
	}
	a.do(get("", "/api/machines/events")).has(t, http.StatusUnauthorized)
}

func TestARowGoesOnlineAndCountsItsTasksOnTheNextRefresh(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	before := p.do(hx(get("Mia", "/api/machines")))
	before.has(t, http.StatusOK, "offline")
	if got := before.header.Get("Vary"); !strings.Contains(got, "HX-Request") {
		t.Errorf("Vary = %q, the same URL answers JSON and a fragment", got)
	}
	m.run(t, p.addr)
	eventually(t, "the row online", func() bool {
		return strings.Contains(rowOf(t, p.do(hx(get("Mia", "/api/machines"))).body, m.id), ">online<")
	})
	id := p.startTask("Fix the checkout")
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	row := rowOf(t, p.do(hx(get("Mia", "/api/machines"))).body, m.id)
	if !strings.Contains(row, `<span class="num">1</span>`) {
		t.Errorf("the row does not count its running task: %s", row)
	}
}
