package main

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"testing"

	"github.com/IniZio/moor/internal/adapter/github/githubtest"
	"github.com/IniZio/moor/internal/adapter/secretbox"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

const (
	hookSecret = "hook-secret"
	prLink     = "https://github.com/example-org/shop-web/pull/90"
)

const evidenceSpec = `name: PR evidence
on:
  pull_request:
    types: [opened, synchronize]
  workflow_dispatch:
concurrency:
  cancel-in-progress: true
prompt: Show evidence for ${{ event.pull_request.html_url }} at ${{ event.pull_request.head.sha }}.
`

const reviewSpec = `name: Code review
on:
  pull_request:
prompt: Review ${{ event.pull_request.title }}.
`

// hookRig is a portal with a GitHub App, a fake GitHub behind it, and the team's home with two pull request specs.
type hookRig struct {
	*app
	fake   *githubtest.Fake
	portal *portal
	cfg    config
}

func newHookRig(t *testing.T, files map[string]string) *hookRig {
	t.Helper()
	fake := githubtest.NewForTest(t)
	key, err := githubtest.NewKey()
	if err != nil {
		t.Fatal(err)
	}
	dsn := pgtest.SchemaDSN(t)
	secret, _ := base64.StdEncoding.DecodeString(testKeyB64)
	if len(secret) != secretbox.KeySize {
		t.Fatal("test key")
	}
	cfg := config{
		listen: "127.0.0.1:0", database: dsn, userHeader: userHeader, orgName: "example-org", secretKey: secret,
		homeDir: writeHome(t, files), publicURL: "https://portal.example.test",
	}
	p, err := assemble(context.Background(), cfg)
	if err != nil {
		t.Fatal(err)
	}
	closeOnce := sync.OnceFunc(p.close)
	t.Cleanup(closeOnce)
	a := &app{t: t, handler: p.handler, dsn: dsn, key: secret, close: closeOnce}
	r := &hookRig{app: a, fake: fake, portal: p, cfg: cfg}
	r.do(put("Mia", 0, appDoc(fake.URL(), key))).has(t, http.StatusOK)
	r.fake.AddPullRequest(githubtest.PullRequest{Repo: "example-org/shop-web", Number: 90, Title: "Add search to the shop", Author: "ana", SHA: "5f0e2b19c4d7", Branch: "search"})
	return r
}

func defaultHome() map[string]string {
	return map[string]string{"tasks/pr-evidence.yaml": evidenceSpec, "tasks/code-review.yaml": reviewSpec}
}

func prBody(action, sha, repo string) string {
	doc := map[string]any{
		"action": action, "number": 90,
		"repository": map[string]any{"full_name": repo, "name": "shop-web", "owner": map[string]any{"login": "example-org"}, "html_url": "https://github.com/" + repo},
		"sender":     map[string]any{"login": "ana"},
		"pull_request": map[string]any{
			"number": 90, "title": "Add search to the shop", "state": "open", "html_url": "https://github.com/" + repo + "/pull/90",
			"user": map[string]any{"login": "ana"},
			"head": map[string]any{"sha": sha, "ref": "search", "repo": map[string]any{"full_name": repo}},
			"base": map[string]any{"ref": "main"},
		},
	}
	b, _ := json.Marshal(doc)
	return string(b)
}

func signed(event, delivery, body, secret string) req {
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write([]byte(body))
	return req{method: http.MethodPost, path: "/hooks/github", json: true, body: body, headers: map[string]string{
		"X-GitHub-Event": event, "X-GitHub-Delivery": delivery, "X-Hub-Signature-256": "sha256=" + hex.EncodeToString(mac.Sum(nil)),
	}}
}

type hookReply struct {
	Outcome   string   `json:"outcome"`
	Duplicate bool     `json:"duplicate"`
	Tasks     []string `json:"tasks"`
}

func (r *hookRig) hook(delivery, action, sha string) hookReply {
	r.t.Helper()
	res := r.do(signed("pull_request", delivery, prBody(action, sha, "example-org/shop-web"), hookSecret))
	res.has(r.t, http.StatusAccepted)
	var out hookReply
	if err := json.Unmarshal([]byte(res.body), &out); err != nil {
		r.t.Fatal(err)
	}
	return out
}

func (r *hookRig) load(id string) domain.Task {
	r.t.Helper()
	_, db := r.tasks()
	got, err := db.Load(context.Background(), domain.TaskID(id))
	if err != nil {
		r.t.Fatal(err)
	}
	return got
}

func (r *hookRig) running(id string) {
	r.t.Helper()
	svc, _ := r.tasks()
	ctx := context.Background()
	if _, err := svc.Report(ctx, domain.TaskID(id), task.Report{Kind: task.SlotTaken, Runner: "build-1"}); err != nil {
		r.t.Fatal(err)
	}
	if _, err := svc.Report(ctx, domain.TaskID(id), task.Report{Kind: task.PartsUp, Run: 1}); err != nil {
		r.t.Fatal(err)
	}
}

func (r *hookRig) publish(id, digest, line string) error {
	_, err := r.portal.answers.Publish(context.Background(), domain.TaskID(id), task.PublishInput{Line: line, EntryURL: "https://results.example/pages/" + id, Digest: digest})
	if err != nil {
		return fmt.Errorf("publish: %w", err)
	}
	return nil
}

func TestASignedPullRequestDeliveryStartsATaskForEachSpecThatHearsIt(t *testing.T) {
	r := newHookRig(t, defaultHome())
	got := r.hook("d-1", "opened", "5f0e2b19c4d7")
	if got.Duplicate || len(got.Tasks) != 2 || !strings.Contains(got.Outcome, "started 2 of 2") {
		t.Fatalf("reply %+v", got)
	}
	byName := map[string]domain.Task{}
	for _, id := range got.Tasks {
		tk := r.load(id)
		byName[string(tk.Spec.Name)] = tk
	}
	ev := byName["PR evidence"]
	if ev.Prompt != "Show evidence for https://github.com/example-org/shop-web/pull/90 at 5f0e2b19c4d7." || ev.StartedBy != "event:github" || ev.State != domain.Waiting {
		t.Errorf("PR evidence task = %+v", ev)
	}
	if ev.Event.Group != "example-org/shop-web#90" || ev.Event.Rev != "5f0e2b19c4d7" || ev.Repo != shopWeb || ev.TokenOwner != "" {
		t.Errorf("event = %+v", ev.Event)
	}
	if byName["Code review"].Prompt != "Review Add search to the shop." {
		t.Errorf("Code review prompt = %q", byName["Code review"].Prompt)
	}
}

func TestASecondDeliveryOfTheSameIdStartsNothing(t *testing.T) {
	r := newHookRig(t, defaultHome())
	first := r.hook("d-1", "opened", "5f0e2b19c4d7")
	again := r.hook("d-1", "opened", "5f0e2b19c4d7")
	if !again.Duplicate || strings.Join(again.Tasks, ",") != strings.Join(first.Tasks, ",") {
		t.Errorf("first %+v, again %+v", first, again)
	}
	if n := len(r.list("Mia", "").Tasks); n != 2 {
		t.Errorf("%d tasks after a redelivery", n)
	}
}

func TestAnotherDeliveryOfTheSameCommitStartsNothingEither(t *testing.T) {
	r := newHookRig(t, defaultHome())
	r.hook("d-1", "opened", "5f0e2b19c4d7")
	r.hook("d-2", "reopened", "5f0e2b19c4d7")
	if n := len(r.list("Mia", "").Tasks); n != 2 {
		t.Errorf("%d tasks, the key of each spec was taken", n)
	}
}

func TestANewHeadCommitSupersedesTheRunningTaskOfASpecThatCancelsInProgress(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": evidenceSpec})
	first := r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks[0]
	r.running(first)
	second := r.hook("d-2", "synchronize", "a1b2c3d4e5f6").Tasks[0]
	if got := r.load(first); got.State != domain.Superseded {
		t.Errorf("the first task is %s", got.State)
	}
	if got := r.load(second); got.State != domain.Waiting || got.Event.Rev != "a1b2c3d4e5f6" {
		t.Errorf("the second task = %+v", got)
	}
}

func TestADeliveryWithTheWrongSignatureStartsNothing(t *testing.T) {
	r := newHookRig(t, defaultHome())
	r.do(signed("pull_request", "d-1", prBody("opened", "5f0e2b19c4d7", "example-org/shop-web"), "another-secret")).has(t, http.StatusUnauthorized)
	if n := len(r.list("Mia", "").Tasks); n != 0 {
		t.Errorf("%d tasks", n)
	}
}

func TestADeliveryOnARepositoryNotInSettingsStartsNothing(t *testing.T) {
	r := newHookRig(t, defaultHome())
	res := r.do(signed("pull_request", "d-1", prBody("opened", "5f0e2b19c4d7", "example-org/other"), hookSecret))
	res.has(t, http.StatusAccepted, "is not under repositories in Settings")
	if n := len(r.list("Mia", "").Tasks); n != 0 {
		t.Errorf("%d tasks", n)
	}
}

func TestOtherEventsAndSpecsThatDoNotListTheActionStartNothing(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": evidenceSpec})
	r.do(signed("ping", "d-1", "{}", hookSecret)).has(t, http.StatusAccepted)
	r.do(signed("pull_request", "d-2", prBody("labeled", "5f0e2b19c4d7", "example-org/shop-web"), hookSecret)).has(t, http.StatusAccepted, "no task spec hears it")
	if n := len(r.list("Mia", "").Tasks); n != 0 {
		t.Errorf("%d tasks", n)
	}
}

func TestWithoutAGitHubAppTheHookRefusesEveryDelivery(t *testing.T) {
	a := newApp(t)
	a.do(signed("pull_request", "d-1", "{}", hookSecret)).has(t, http.StatusServiceUnavailable, "no GitHub App")
}

// appDoc is the settings with the App typed in: its key and webhook secret, which the save seals.
func appDoc(api string, key []byte) string {
	indented := "        " + strings.ReplaceAll(strings.TrimSpace(string(key)), "\n", "\n        ")
	return "repositories:\n  - example-org/shop-web\ncredentials:\n  github:\n    github_app:\n      app_id: 17\n" +
		"      private_key: |\n" + indented + "\n      webhook_secret: " + hookSecret + "\n      api_url: " + api + "/\n"
}

func TestTheAppMovesWithTheSettings(t *testing.T) {
	r := newHookRig(t, defaultHome())
	r.do(signed("ping", "d-1", "{}", hookSecret)).has(t, http.StatusAccepted)
	cur := r.current()
	r.do(put("Mia", cur.Version, "repositories:\n  - example-org/shop-web\n")).has(t, http.StatusOK)
	r.do(signed("ping", "d-2", "{}", hookSecret)).has(t, http.StatusServiceUnavailable)
}

// writes counts the comments the fake GitHub was asked to create and to edit.
func (r *hookRig) writes() (creates, edits int) {
	return r.fake.Count(http.MethodPost, "/repos/example-org/shop-web/issues/90/comments"), r.fake.Count(http.MethodPatch, "/repos/example-org/shop-web/issues/comments/")
}

func (r *hookRig) wantWrites(creates, edits int) {
	r.t.Helper()
	if c, e := r.writes(); c != creates || e != edits {
		r.t.Fatalf("%d creates and %d edits, want %d and %d", c, e, creates, edits)
	}
}

func TestPublishPostsOneCommentAndTheNextPublishEditsIt(t *testing.T) {
	r := newHookRig(t, defaultHome())
	ids := r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks
	for _, id := range ids {
		r.running(id)
	}
	if err := r.publish(ids[0], "d1", "Five of five flows passed."); err != nil {
		t.Fatal(err)
	}
	if got := r.load(ids[0]); got.State != domain.Done {
		t.Fatalf("the task is %s", got.State)
	}
	r.wantWrites(1, 0)
	if err := r.publish(ids[1], "d2", "Looks fine."); err != nil {
		t.Fatal(err)
	}
	r.wantWrites(1, 1)
	comments := r.fake.Comments()
	if len(comments) != 1 || !strings.Contains(comments[0].Body, "Five of five flows passed.") || !strings.Contains(comments[0].Body, "Looks fine.") {
		t.Fatalf("comments = %+v", comments)
	}
	if !strings.Contains(r.runPage(ids[1]), "Commented on pull request #90") {
		t.Errorf("the Run page does not say the task commented")
	}
}

func (r *hookRig) runPage(id string) string {
	r.t.Helper()
	res := r.do(get("Mia", "/tasks/"+id))
	res.has(r.t, http.StatusOK)
	return res.body
}

// errorRows lists the data of the error rows of a task.
func (r *hookRig) errorRows(id string) []string {
	r.t.Helper()
	_, db := r.tasks()
	rows, err := db.EventsAfter(context.Background(), domain.TaskID(id), 0, 100)
	if err != nil {
		r.t.Fatal(err)
	}
	var out []string
	for _, row := range rows {
		if row.Kind == domain.KindError {
			out = append(out, string(row.Data))
		}
	}
	return out
}

func TestAFailedPostIsRecordedOnTheTaskAndTheSamePublishTriesItAgain(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": evidenceSpec})
	id := r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks[0]
	r.running(id)
	r.fake.FailPosts(1)
	if err := r.publish(id, "d1", "Five of five flows passed."); err == nil {
		t.Fatal("the failed post was not reported")
	}
	if got := r.load(id); got.State != domain.Delivering || len(r.fake.Comments()) != 0 {
		t.Fatalf("task %s, %d comments", got.State, len(r.fake.Comments()))
	}
	reasons := r.errorRows(id)
	if len(reasons) != 1 || !strings.Contains(reasons[0], "the answer for version 1 was not posted") || !strings.Contains(reasons[0], "500") {
		t.Fatalf("error rows = %v", reasons)
	}
	r.wantWrites(1, 0)
	if err := r.publish(id, "d1", "Five of five flows passed."); err != nil {
		t.Fatalf("the second try: %v", err)
	}
	if got := r.load(id); got.State != domain.Done || len(r.fake.Comments()) != 1 {
		t.Errorf("task %s, %d comments", got.State, len(r.fake.Comments()))
	}
}

func TestRunOnALinkStartsTheTaskTheWebhookWould(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": evidenceSpec})
	hooked := r.load(r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks[0])
	res := r.do(req{method: http.MethodPost, path: "/api/task-specs/PR%20evidence/run", person: "Ken", json: true, body: `{"input":"` + prLink + `"}`})
	res.has(t, http.StatusOK)
	var out struct{ Task struct{ ID string } }
	if err := json.Unmarshal([]byte(res.body), &out); err != nil {
		t.Fatal(err)
	}
	ran := r.load(out.Task.ID)
	if ran.ID == hooked.ID || ran.Prompt != hooked.Prompt || ran.Repo != hooked.Repo || ran.Event.Group != hooked.Event.Group || ran.Spec.Name != hooked.Spec.Name ||
		ran.Ref != hooked.Ref || ran.Commit != hooked.Commit {
		t.Errorf("run task %+v, webhook task %+v", ran, hooked)
	}
	if ran.StartedBy != "Ken" || ran.TokenOwner != "Ken" || !strings.HasPrefix(string(ran.Event.Rev), "5f0e2b19c4d7+run-") {
		t.Errorf("started by %q, token %q, rev %q", ran.StartedBy, ran.TokenOwner, ran.Event.Rev)
	}
	form := hx(req{method: http.MethodPost, path: "/api/task-specs/PR%20evidence/run", person: "Ken", form: url.Values{"input": {prLink}}})
	r.do(form).has(t, http.StatusOK, "Started task #3 on example-org/shop-web.")
	if n := len(r.list("Mia", "").Tasks); n != 3 {
		t.Errorf("%d tasks, a Run does not take the key of an event", n)
	}
}

func TestRunRefusesWhatItCannotStart(t *testing.T) {
	r := newHookRig(t, defaultHome())
	run := func(spec, input string) reply {
		b, _ := json.Marshal(map[string]string{"input": input})
		return r.do(req{method: http.MethodPost, path: "/api/task-specs/" + url.PathEscape(spec) + "/run", person: "Ken", json: true, body: string(b)})
	}
	run("PR evidence", "https://github.com/example-org/shop-web").has(t, http.StatusUnprocessableEntity, "Paste a pull request link, such as https://github.com/example-org/shop-web/pull/91.")
	run("PR evidence", "not a link").has(t, http.StatusUnprocessableEntity, "Paste a pull request link")
	r.fake.AddPullRequest(githubtest.PullRequest{Repo: "example-org/other", Number: 3, Title: "x", Author: "ana", SHA: "abc"})
	run("PR evidence", "https://github.com/example-org/other/pull/3").has(t, http.StatusUnprocessableEntity, "example-org/other is not under repositories in Settings. Add it there first.")
	run("Code review", prLink).has(t, http.StatusConflict, "cannot be started by hand")
	run("No such spec", prLink).has(t, http.StatusNotFound)
	if n := len(r.list("Mia", "").Tasks); n != 0 {
		t.Errorf("%d tasks", n)
	}
}

func TestEventTasksAreTitledByTheirPullRequest(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": evidenceSpec})
	id := r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks[0]
	if got := r.list("Mia", "").Tasks[0].Title; got != "Add search to the shop" {
		t.Errorf("title = %q", got)
	}
	page := r.do(get("Mia", "/tasks")).body
	for _, want := range []string{"Add search to the shop", "pull request #90 opened by ana"} {
		if !strings.Contains(page, want) {
			t.Errorf("the Tasks page lacks %q", want)
		}
	}
	run := r.runPage(id)
	for _, want := range []string{"example-org/shop-web, pull request #90", "ana opened pull request #90", `href="https://github.com/example-org/shop-web/pull/90"`} {
		if !strings.Contains(run, want) {
			t.Errorf("the Run page lacks %q", want)
		}
	}
	ran := r.do(req{method: http.MethodPost, path: "/api/task-specs/PR%20evidence/run", person: "Ken", json: true, body: `{"input":"` + prLink + `"}`})
	ran.has(t, http.StatusOK)
	if !strings.Contains(r.do(get("Mia", "/tasks")).body, "pull request #90, Run by Ken") {
		t.Errorf("the Tasks page does not say who ran it")
	}
}

func TestTheSpecBannerOffersRunToASpecWithWorkflowDispatch(t *testing.T) {
	r := newHookRig(t, defaultHome())
	page := r.do(get("Mia", "/tasks?spec="+url.QueryEscape("PR evidence"))).body
	for _, want := range []string{"<summary class=\"btn\">Run</summary>", "Pull request link", `hx-post="/api/task-specs/PR%20evidence/run"`} {
		if !strings.Contains(page, want) {
			t.Errorf("the spec page lacks %q", want)
		}
	}
	if strings.Contains(r.do(get("Mia", "/tasks?spec="+url.QueryEscape("Code review"))).body, "runmenu") {
		t.Errorf("a spec without workflow_dispatch has Run")
	}
}

func TestASpecWhosePromptCannotBeFilledIsPassedOverAndTheOthersStart(t *testing.T) {
	broken := "name: Broken\non:\n  pull_request:\nprompt: Read ${{ event.pull_request.nope }}.\n"
	r := newHookRig(t, map[string]string{"tasks/broken.yaml": broken, "tasks/code-review.yaml": reviewSpec})
	got := r.hook("d-1", "opened", "5f0e2b19c4d7")
	if len(got.Tasks) != 1 || !strings.Contains(got.Outcome, "started 1 of 2") || !strings.Contains(got.Outcome, "Broken") {
		t.Errorf("reply %+v", got)
	}
}
