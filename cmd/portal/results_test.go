package main

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/web"
)

const reviewedSpec = `name: PR evidence
on:
  pull_request:
    types: [opened]
prompt: Show evidence for ${{ event.pull_request.html_url }}.
review: Check your page against the check list.
`

type received struct {
	Version int    `json:"version"`
	URL     string `json:"url"`
	State   string `json:"state"`
}

// taskToken is the token the runner hands the desk of a task.
func (a *app) taskToken(id string) string { return newTokens(a.key).mint(domain.TaskID(id), taskScope) }

// publishReq is the publish tool's request: the page and its one line, under the key of one run of the tool.
func publishReq(id, token, key, line, page string) req {
	body, _ := json.Marshal(map[string]string{"line": line, "page": page, "key": key})
	r := req{method: http.MethodPost, path: "/api/tasks/" + id + "/results", json: true, body: string(body)}
	if token != "" {
		r.headers = map[string]string{"Authorization": "Bearer " + token}
	}
	return r
}

func decodeReceipt(t *testing.T, r reply) received {
	t.Helper()
	var out received
	if err := json.Unmarshal([]byte(r.body), &out); err != nil {
		t.Fatalf("%v in %s", err, r.body)
	}
	return out
}

// runningPlain starts a plain-prompt task and brings it to running through the task service.
func runningPlain(t *testing.T, a *app, review string) string {
	t.Helper()
	svc, db := a.tasks()
	id, err := db.NextTaskID(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := svc.StartByPerson(context.Background(), task.StartInput{ID: id, Repo: shopWeb, Prompt: "Fix the checkout", ReviewPrompt: review, By: "Mia"}); err != nil {
		t.Fatal(err)
	}
	for _, r := range []task.Report{{Kind: task.SlotTaken, Runner: "build-1"}, {Kind: task.PartsUp, Run: 1}} {
		if _, err := svc.Report(context.Background(), id, r); err != nil {
			t.Fatal(err)
		}
	}
	return string(id)
}

func TestOnlyTheTasksOwnTokenPublishesAResult(t *testing.T) {
	a := newApp(t, func(c *config) { c.publicURL = "https://portal.example.test" })
	a.withRepos()
	mine, other := runningPlain(t, a, ""), runningPlain(t, a, "")
	deskProxy := newTokens(a.key).mint(domain.TaskID(mine), domain.DeskProxy)
	boxProxy := newTokens(a.key).mint(domain.TaskID(mine), domain.BoxProxy)
	for name, r := range map[string]req{
		"a person with no token":     publishReq(mine, "", "k1", "ok", "<p>x</p>"),
		"a person's own session":     {method: http.MethodPost, path: "/api/tasks/" + mine + "/results", person: "Mia", json: true, body: `{"line":"ok","page":"<p>x</p>"}`},
		"a made-up token":            publishReq(mine, "task.AAAA", "k1", "ok", "<p>x</p>"),
		"the desk proxy's token":     publishReq(mine, deskProxy, "k1", "ok", "<p>x</p>"),
		"the Box proxy's token":      publishReq(mine, boxProxy, "k1", "ok", "<p>x</p>"),
		"another task's task token":  publishReq(mine, a.taskToken(other), "k1", "ok", "<p>x</p>"),
		"a task that does not exist": publishReq("nope", "task.AAAA", "k1", "ok", "<p>x</p>"),
	} {
		got := a.do(r)
		if got.code != http.StatusUnauthorized {
			t.Errorf("%s: status %d, want 401: %s", name, got.code, got.body)
		}
	}
	_, db := a.tasks()
	if v, _ := db.Versions(context.Background(), domain.TaskID(mine)); len(v) != 0 {
		t.Fatalf("a refused call left %d versions", len(v))
	}
	ok := a.do(publishReq(mine, a.taskToken(mine), "k1", "Five of five flows passed.", "<h1>Result</h1>"))
	ok.has(t, http.StatusOK)
	got := decodeReceipt(t, ok)
	if got.Version != 1 || got.URL != "https://portal.example.test/results/"+mine+"/1" || got.State != "done" {
		t.Fatalf("receipt %+v", got)
	}
}

func TestAPublishedPageIsServedSandboxedToASignedInPersonOnly(t *testing.T) {
	a := newApp(t)
	a.withRepos()
	id := runningPlain(t, a, "")
	page := "<h1>Result</h1><script>document.title = 'ran'</script>"
	a.do(publishReq(id, a.taskToken(id), "k1", "ok", page)).has(t, http.StatusOK)
	path := "/results/" + id + "/1"
	res := a.do(get("Mia", path))
	res.has(t, http.StatusOK)
	if res.body != page {
		t.Errorf("body %q, want the page as published", res.body)
	}
	csp := res.header.Get("Content-Security-Policy")
	for _, want := range []string{"sandbox", "default-src 'none'"} {
		if !strings.Contains(csp, want) {
			t.Errorf("Content-Security-Policy %q lacks %q", csp, want)
		}
	}
	if strings.Contains(csp, "allow-same-origin") || !strings.HasPrefix(res.header.Get("Content-Type"), "text/html") || res.header.Get("X-Content-Type-Options") != "nosniff" {
		t.Errorf("headers %v", res.header)
	}
	a.do(get("", path)).has(t, http.StatusUnauthorized)
	a.do(get("Mia", "/results/"+id+"/2")).has(t, http.StatusNotFound)
	a.do(get("Mia", "/results/"+id+"/one")).has(t, http.StatusNotFound)
	a.do(get("Mia", "/results/nope/1")).has(t, http.StatusNotFound)
}

func TestAPageOverTheSizeCapIsRefusedAndStoresNothing(t *testing.T) {
	a := newApp(t)
	a.withRepos()
	id := runningPlain(t, a, "")
	big := strings.Repeat("a", web.MaxPageBytes+1)
	a.do(publishReq(id, a.taskToken(id), "k1", "ok", big)).has(t, http.StatusRequestEntityTooLarge)
	_, db := a.tasks()
	if v, _ := db.Versions(context.Background(), domain.TaskID(id)); len(v) != 0 {
		t.Fatalf("%d versions", len(v))
	}
	a.do(publishReq(id, a.taskToken(id), "k1", "ok", big[1:])).has(t, http.StatusOK)
	a.do(publishReq(id, a.taskToken(id), "k2", "", "<p>x</p>")).has(t, http.StatusBadRequest)
	a.do(publishReq(id, a.taskToken(id), "k2", "ok", "")).has(t, http.StatusBadRequest)
	a.do(publishReq(id, a.taskToken(id), "k2", "two\nlines", "<p>x</p>")).has(t, http.StatusBadRequest)
}

func TestTheSameKeyIsTheSameVersionAndANewKeyIsANewOne(t *testing.T) {
	a := newApp(t)
	a.withRepos()
	id := runningPlain(t, a, "check it")
	tok := a.taskToken(id)
	first := decodeReceipt(t, a.do(publishReq(id, tok, "k1", "ok", "<p>same</p>")))
	again := decodeReceipt(t, a.do(publishReq(id, tok, "k1", "ok", "<p>same</p>")))
	if first.Version != 1 || again.Version != 1 || again.State != "running" {
		t.Fatalf("first %+v, again %+v", first, again)
	}
	second := decodeReceipt(t, a.do(publishReq(id, tok, "k2", "ok", "<p>same</p>")))
	if second.Version != 2 || second.State != "done" || second.URL == first.URL {
		t.Fatalf("a new run of the tool with the same page: %+v", second)
	}
	a.do(publishReq(id, tok, "k3", "ok", "<p>late</p>")).has(t, http.StatusConflict)
}

func TestAnEventTasksVersionsPostOneCommentThenEditIt(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": reviewedSpec})
	id := r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks[0]
	r.running(id)
	tok := r.taskToken(id)
	first := decodeReceipt(t, r.do(publishReq(id, tok, "k1", "Four of five flows passed.", "<h1>v1</h1>")))
	if first.State != "running" {
		t.Fatalf("the first version of a reviewed task: %+v", first)
	}
	r.wantWrites(0, 0)
	second := decodeReceipt(t, r.do(publishReq(id, tok, "k2", "Five of five flows passed.", "<h1>v2</h1>")))
	if second.Version != 2 || second.State != "done" {
		t.Fatalf("the accepted version: %+v", second)
	}
	r.wantWrites(1, 0)
	comments := r.fake.Comments()
	if len(comments) != 1 || !strings.Contains(comments[0].Body, "Five of five flows passed.") || !strings.Contains(comments[0].Body, "/results/"+id+"/2") {
		t.Fatalf("comments = %+v", comments)
	}
	if strings.Contains(comments[0].Body, "Four of five") {
		t.Errorf("the comment shows the version held for review: %s", comments[0].Body)
	}
}

// typedReviews counts the times the review prompt was typed into the machine's fake session.
func typedReviews(m *machineRig) int {
	screen, _ := m.sessions.Screen(context.Background(), "", 0)
	return strings.Count(screen, "check it")
}

func reviewNotes(t *testing.T, db *store.Store, id string) int {
	t.Helper()
	rows, err := db.EventsAfter(context.Background(), domain.TaskID(id), 0, 200)
	if err != nil {
		t.Fatal(err)
	}
	n := 0
	for _, row := range rows {
		if row.Kind == domain.KindNote && strings.Contains(string(row.Data), domain.PhaseReview) {
			n++
		}
	}
	return n
}

func TestTheReviewPromptIsTypedOnceOnTwoReplicasAcrossARestart(t *testing.T) {
	dsn := pgtest.SchemaDSN(t)
	linked := startReplica(t, dsn, "127.0.0.1:0")
	other := startReplica(t, dsn, "127.0.0.1:0")
	linked.withRepos()
	m := enrol(t, linked, "host-a")
	m.run(t, linked.addr)
	eventually(t, "the machine linked", func() bool { return linked.machines("Mia")[m.id].State == "linked" })
	svc, db := linked.tasks()
	tid, err := db.NextTaskID(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := svc.StartByPerson(context.Background(), task.StartInput{ID: tid, Repo: shopWeb, Prompt: "Fix the checkout", ReviewPrompt: "check it", By: "Mia"}); err != nil {
		t.Fatal(err)
	}
	id := string(tid)
	eventually(t, "the task running", stateIs(t, db, tid, domain.Running))
	time.Sleep(300 * time.Millisecond)
	if n := typedReviews(m); n != 0 {
		t.Fatalf("the review prompt was typed %d times before anything was published", n)
	}

	other.do(publishReq(id, other.taskToken(id), "k1", "Four of five.", "<p>1</p>")).has(t, http.StatusOK)
	eventually(t, "the review prompt typed", func() bool { return typedReviews(m) == 1 })
	time.Sleep(500 * time.Millisecond)

	addr := linked.addr
	linked.stop()
	again := startReplica(t, dsn, addr)
	eventually(t, "the runner linked again", func() bool { return again.machines("Mia")[m.id].State == "linked" })
	time.Sleep(500 * time.Millisecond)
	if n := typedReviews(m); n != 1 || reviewNotes(t, db, id) != 1 {
		t.Fatalf("after a replica restart the review prompt was typed %d times, with %d review notes", n, reviewNotes(t, db, id))
	}
	if got, _ := db.Load(context.Background(), tid); got.State != domain.Running {
		t.Fatalf("the task is %s while it waits for its second version", got.State)
	}
	second := decodeReceipt(t, other.do(publishReq(id, other.taskToken(id), "k2", "Five of five.", "<p>2</p>")))
	if second.Version != 2 || second.State != "done" {
		t.Fatalf("the second version: %+v", second)
	}
	if n := typedReviews(m); n != 1 {
		t.Fatalf("the review prompt was typed %d times in all", n)
	}
}

func TestTheRunnersNotesBecomeTheSetUpNotesOfTheTask(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	id := p.startTask("Fix the checkout")
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	want := []string{domain.NoteText(domain.PhaseDesk, m.id), domain.PhaseBox, domain.PhasePrompt}
	eventually(t, "the runner's notes on the task", func() bool {
		rows, err := db.EventsAfter(context.Background(), id, 0, 200)
		if err != nil {
			t.Fatal(err)
		}
		var got []string
		for _, r := range rows {
			var n domain.NoteData
			if r.Kind == domain.KindNote && json.Unmarshal(r.Data, &n) == nil && n.Text != "" && !strings.HasPrefix(n.Text, "retry") {
				got = append(got, n.Text)
			}
		}
		return strings.Join(got, "|") == strings.Join(want, "|")
	})
	page := p.do(get("Mia", "/tasks/"+string(id)+"/setup"))
	page.has(t, http.StatusOK, "Start desk and Box", "desk up: "+m.id)
}
