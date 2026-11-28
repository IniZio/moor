package github_test

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/github/githubtest"
	"github.com/IniZio/moor/internal/domain"
)

const secret = "webhook-secret"

type sink struct {
	got []domain.Inbound
	err error
}

func (s *sink) Receive(_ context.Context, in domain.Inbound) (domain.InboundResult, error) {
	s.got = append(s.got, in)
	return domain.InboundResult{Tasks: []domain.TaskID{"7"}, Outcome: "started 1 task"}, s.err
}

type rig struct {
	fake    *githubtest.Fake
	adapter *github.Adapter
	sink    *sink
	hook    http.Handler
}

func newRig(t *testing.T) *rig {
	t.Helper()
	fake := githubtest.NewForTest(t)
	key, err := githubtest.NewKey()
	if err != nil {
		t.Fatal(err)
	}
	a, err := github.New(github.Config{AppID: 17, Key: key, WebhookSecret: []byte(secret), APIBase: fake.URL()})
	if err != nil {
		t.Fatal(err)
	}
	s := &sink{}
	return &rig{fake: fake, adapter: a, sink: s, hook: a.Hook(s)}
}

func prPayload(action string, mutate func(map[string]any)) []byte {
	doc := map[string]any{
		"action": action, "number": 90,
		"repository": map[string]any{"full_name": "example-org/shop-web", "name": "shop-web", "owner": map[string]any{"login": "example-org"}, "html_url": "https://github.com/example-org/shop-web"},
		"sender":     map[string]any{"login": "ana"},
		"pull_request": map[string]any{
			"number": 90, "title": "Add search to the shop", "body": "Adds a search box.", "state": "open", "draft": false,
			"html_url": "https://github.com/example-org/shop-web/pull/90",
			"user":     map[string]any{"login": "ana"},
			"head":     map[string]any{"sha": "5f0e2b19c4d7", "ref": "search", "repo": map[string]any{"full_name": "example-org/shop-web"}},
			"base":     map[string]any{"ref": "main"},
		},
	}
	if mutate != nil {
		mutate(doc)
	}
	b, _ := json.Marshal(doc)
	return b
}

func deliver(h http.Handler, event, id string, body []byte, sign string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(http.MethodPost, "/hooks/github", bytes.NewReader(body))
	r.Header.Set("Content-Type", "application/json")
	r.Header.Set("X-GitHub-Event", event)
	r.Header.Set("X-GitHub-Delivery", id)
	mac := hmac.New(sha256.New, []byte(sign))
	mac.Write(body)
	r.Header.Set("X-Hub-Signature-256", "sha256="+hex.EncodeToString(mac.Sum(nil)))
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, r)
	return rec
}

func TestASignedPullRequestDeliveryIsReadIntoTheDomainsEvent(t *testing.T) {
	r := newRig(t)
	rec := deliver(r.hook, "pull_request", "d-1", prPayload("opened", nil), secret)
	if rec.Code != http.StatusAccepted || len(r.sink.got) != 1 || r.sink.got[0].Occurrence == nil {
		t.Fatalf("status %d, the sink got %+v", rec.Code, r.sink.got)
	}
	in := r.sink.got[0]
	if in.DeliveryID != "d-1" || in.Source != "github" || in.Action != "opened" {
		t.Errorf("inbound %+v", in)
	}
	occ := in.Occurrence
	if occ.Event.Source != "github" || occ.Event.Group != "example-org/shop-web#90" || occ.Event.Rev != "5f0e2b19c4d7" {
		t.Errorf("event %+v", occ.Event)
	}
	if occ.Incoming.Repo.String() != "github.com/example-org/shop-web" || occ.Incoming.Name != domain.EventPullRequest || occ.Incoming.Action != "opened" {
		t.Errorf("incoming %+v", occ.Incoming)
	}
}

func TestThePromptTemplateReadsGitHubsOwnFieldNamesFromTheEvent(t *testing.T) {
	r := newRig(t)
	deliver(r.hook, "pull_request", "d-1", prPayload("opened", nil), secret)
	got, err := domain.FillTemplate("Show evidence for ${{ event.pull_request.html_url }} at ${{ event.pull_request.head.sha }}", r.sink.got[0].Occurrence.Incoming.Payload, nil)
	if err != nil || got != "Show evidence for https://github.com/example-org/shop-web/pull/90 at 5f0e2b19c4d7" {
		t.Errorf("prompt %q, %v", got, err)
	}
}

func TestADeliveryWithTheWrongSignatureIsRefused(t *testing.T) {
	r := newRig(t)
	for name, sign := range map[string]string{"wrong secret": "another-secret"} {
		if rec := deliver(r.hook, "pull_request", "d-1", prPayload("opened", nil), sign); rec.Code != http.StatusUnauthorized {
			t.Errorf("%s: status %d", name, rec.Code)
		}
	}
	unsigned := httptest.NewRequest(http.MethodPost, "/hooks/github", strings.NewReader("{}"))
	unsigned.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	r.hook.ServeHTTP(rec, unsigned)
	if rec.Code != http.StatusUnauthorized || len(r.sink.got) != 0 {
		t.Errorf("unsigned: status %d, sink %v", rec.Code, r.sink.got)
	}
}

func TestOtherEventsAreAcceptedAndSetAside(t *testing.T) {
	r := newRig(t)
	for _, event := range []string{"ping", "push", "issues"} {
		if rec := deliver(r.hook, event, "d-"+event, []byte("{}"), secret); rec.Code != http.StatusAccepted {
			t.Errorf("%s: status %d", event, rec.Code)
		}
	}
	if len(r.sink.got) != 0 {
		t.Errorf("the sink got %+v", r.sink.got)
	}
}

func TestADraftPullRequestIsSetAside(t *testing.T) {
	r := newRig(t)
	body := prPayload("opened", func(d map[string]any) { d["pull_request"].(map[string]any)["draft"] = true })
	deliver(r.hook, "pull_request", "d-1", body, secret)
	if len(r.sink.got) != 1 || r.sink.got[0].Occurrence != nil || !strings.Contains(r.sink.got[0].Ignored, "draft") {
		t.Errorf("the sink got %+v", r.sink.got)
	}
}

func TestAForkPullRequestStartsOnlyWhenItsAuthorCanWrite(t *testing.T) {
	fork := func(d map[string]any) {
		head := d["pull_request"].(map[string]any)["head"].(map[string]any)
		head["repo"] = map[string]any{"full_name": "ana/shop-web"}
	}
	for level, started := range map[string]bool{"write": true, "admin": true, "read": false, "none": false} {
		r := newRig(t)
		r.fake.SetPermission("example-org/shop-web", "ana", level)
		deliver(r.hook, "pull_request", "d-1", prPayload("opened", fork), secret)
		if got := len(r.sink.got) == 1 && r.sink.got[0].Occurrence != nil; got != started {
			t.Errorf("author with %s: started %v, want %v (%+v)", level, got, started, r.sink.got)
		}
	}
}

func TestAFailedStoreIsAServerErrorSoGitHubSendsItAgain(t *testing.T) {
	r := newRig(t)
	r.sink.err = errors.New("database down")
	if rec := deliver(r.hook, "pull_request", "d-1", prPayload("opened", nil), secret); rec.Code != http.StatusInternalServerError {
		t.Errorf("status %d", rec.Code)
	}
}

func TestRunOnALinkReadsTheSameEventAsTheWebhook(t *testing.T) {
	r := newRig(t)
	deliver(r.hook, "pull_request", "d-1", prPayload("opened", nil), secret)
	r.fake.AddPullRequest(githubtest.PullRequest{Repo: "example-org/shop-web", Number: 90, Title: "Add search to the shop", Body: "Adds a search box.", Author: "ana", SHA: "5f0e2b19c4d7", Branch: "search"})
	run, err := r.adapter.Occurrence(context.Background(), "https://github.com/example-org/shop-web/pull/90")
	if err != nil {
		t.Fatal(err)
	}
	hooked := r.sink.got[0].Occurrence
	if run.Event.Group != hooked.Event.Group || run.Event.Rev != hooked.Event.Rev || run.Incoming.Repo != hooked.Incoming.Repo {
		t.Errorf("run %+v, webhook %+v", run.Event, hooked.Event)
	}
	if run.Incoming.Name != domain.EventWorkflowDispatch {
		t.Errorf("a Run is a %s event", run.Incoming.Name)
	}
	prompt := "${{ event.pull_request.html_url }} ${{ event.pull_request.head.sha }}"
	a, _ := domain.FillTemplate(prompt, run.Incoming.Payload, nil)
	b, _ := domain.FillTemplate(prompt, hooked.Incoming.Payload, nil)
	if a != b {
		t.Errorf("prompts differ: %q and %q", a, b)
	}
}

func TestRunRefusesALinkThatIsNotAPullRequest(t *testing.T) {
	r := newRig(t)
	for _, link := range []string{"", "https://github.com/example-org/shop-web", "https://github.com/example-org/shop-web/issues/9", "https://example.com/example-org/shop-web/pull/9", "https://github.com/example-org/shop-web/pull/x", "javascript:alert(1)"} {
		if _, err := r.adapter.Occurrence(context.Background(), link); !errors.Is(err, github.ErrNotPullRequestLink) {
			t.Errorf("%q gave %v", link, err)
		}
	}
}

func TestTheScreensReadATitleAndWhoOpenedIt(t *testing.T) {
	r := newRig(t)
	deliver(r.hook, "pull_request", "d-1", prPayload("synchronize", nil), secret)
	got, ok := r.adapter.ReadEvent(r.sink.got[0].Occurrence.Event)
	want := domain.EventText{Title: "Add search to the shop", Subject: "pull request #90", Verb: "updated", Actor: "ana", URL: "https://github.com/example-org/shop-web/pull/90"}
	if !ok || got != want {
		t.Errorf("read %+v, %v, want %+v", got, ok, want)
	}
	if _, ok := r.adapter.ReadEvent(domain.Event{Source: "github", Context: []byte("{}")}); ok {
		t.Error("an event with no pull request was read")
	}
}

func TestOneCommentIsPostedThenEditedAndFoundByItsMarker(t *testing.T) {
	r := newRig(t)
	deliver(r.hook, "pull_request", "d-1", prPayload("opened", nil), secret)
	ev := r.sink.got[0].Occurrence.Event
	target, ok := r.adapter.Target(ev)
	if !ok || target.Label != "pull request #90" || target.Ref != "example-org/shop-web#90" {
		t.Fatalf("target %+v, %v", target, ok)
	}
	ctx := context.Background()
	if _, found, err := r.adapter.Find(ctx, target); err != nil || found {
		t.Fatalf("found a comment before any: %v %v", found, err)
	}
	d := domain.Delivery{Ref: target.Ref, Sections: map[domain.SpecName]domain.Section{
		"PR evidence": {Task: "7", Rev: "5f0e2b19c4d7", Version: 1, Line: "Five of five flows passed.", URL: "https://results.example/pages/7"},
	}}
	id, err := r.adapter.Post(ctx, target, r.adapter.Body(d))
	if err != nil {
		t.Fatal(err)
	}
	d.Sections["Dependency check"] = domain.Section{Task: "8", Rev: "5f0e2b19c4d7", Line: "No outdated packages.", URL: "javascript:alert(1)"}
	if err := r.adapter.Edit(ctx, target, id, r.adapter.Body(d)); err != nil {
		t.Fatal(err)
	}
	found, ok, err := r.adapter.Find(ctx, target)
	if err != nil || !ok || found != id {
		t.Fatalf("found %q %v %v, want %q", found, ok, err, id)
	}
	comments := r.fake.Comments()
	if len(comments) != 1 {
		t.Fatalf("%d comments", len(comments))
	}
	wantBody(t, comments[0].Body)
}

func wantBody(t *testing.T, body string) {
	t.Helper()
	for _, want := range []string{"<!-- moor:answer example-org/shop-web#90 -->", "<!-- moor:section spec=PR+evidence rev=5f0e2b19c4d7 -->", "**PR evidence** at `5f0e2b1`: Five of five flows passed.", "[Result page](https://results.example/pages/7)", "**Dependency check**"} {
		if !strings.Contains(body, want) {
			t.Errorf("the comment lacks %q:\n%s", want, body)
		}
	}
	if strings.Contains(body, "javascript:") {
		t.Errorf("the comment links a script:\n%s", body)
	}
	if strings.Index(body, "Dependency check") > strings.Index(body, "PR evidence") {
		t.Errorf("the sections are not in the order of their specs' names:\n%s", body)
	}
}

func TestAnAppWithOneMissingPartIsRefused(t *testing.T) {
	if _, err := github.New(github.Config{AppID: 17, WebhookSecret: []byte("s")}); err == nil || !strings.Contains(err.Error(), "private key") || !strings.Contains(err.Error(), "API base URL") {
		t.Errorf("got %v", err)
	}
}

func TestAnAppWithNoWebhookSecretRefusesEveryDelivery(t *testing.T) {
	fake := githubtest.NewForTest(t)
	key, err := githubtest.NewKey()
	if err != nil {
		t.Fatal(err)
	}
	a, err := github.New(github.Config{AppID: 17, Key: key, APIBase: fake.URL()})
	if err != nil {
		t.Fatal(err)
	}
	s := &sink{}
	if w := deliver(a.Hook(s), "pull_request", "d-1", prPayload("opened", nil), ""); w.Code != http.StatusServiceUnavailable || len(s.got) != 0 {
		t.Fatalf("an unsigned delivery answered %d and reached the sink %d times", w.Code, len(s.got))
	}
}

func TestATaskTokenIsLimitedToItsRepository(t *testing.T) {
	r := newRig(t)
	token, expires, err := r.adapter.TaskToken(context.Background(), "example-org/shop-web")
	if err != nil || token != "fake-token-4242" || time.Until(expires) < 50*time.Minute {
		t.Fatalf("token %q expires %v: %v", token, expires, err)
	}
	got := r.fake.TokenRequests()
	want := []githubtest.TokenRequest{{
		Installation: "4242", Repositories: []string{"shop-web"},
		Permissions: map[string]string{"contents": "write", "pull_requests": "write", "issues": "write"},
	}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("token requests %+v, want %+v", got, want)
	}
	if _, _, err := r.adapter.TaskToken(context.Background(), "shop-web"); err == nil {
		t.Error("a repository with no owner got a token")
	}
}
