package main

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/tmaxmax/go-sse"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// peer builds a second portal on the database of a, as another replica would be.
func (a *app) peer() *app {
	a.t.Helper()
	handler, closeApp, err := build(context.Background(), config{listen: "127.0.0.1:0", database: a.dsn, userHeader: userHeader, orgName: "example-org", secretKey: a.key})
	if err != nil {
		a.t.Fatalf("build a peer: %v", err)
	}
	a.t.Cleanup(closeApp)
	return &app{t: a.t, handler: handler, dsn: a.dsn, key: a.key}
}

func (a *app) store() *store.Store {
	a.t.Helper()
	db, err := store.Open(context.Background(), a.dsn)
	if err != nil {
		a.t.Fatal(err)
	}
	a.t.Cleanup(db.Close)
	return db
}

func (a *app) service() *task.Service { return task.New(a.store(), systemClock{}, task.Config{}) }

// play appends rows to a stored task as the portal's own writers would, one write each.
func (a *app) play(id string, rows ...domain.EventRow) {
	a.t.Helper()
	ctx := context.Background()
	db := a.store()
	for _, r := range rows {
		t, err := db.Load(ctx, domain.TaskID(id))
		if err != nil {
			a.t.Fatal(err)
		}
		r.Task, r.Seq = t.ID, t.LastSeq+1
		t.LastSeq = r.Seq
		if r.Kind == domain.KindState {
			var d domain.StateData
			if err := json.Unmarshal(r.Data, &d); err != nil {
				a.t.Fatal(err)
			}
			t.State, _ = domain.ParseState(d.To)
		}
		if _, err := db.Save(ctx, domain.TaskWrite{Task: t, Events: []domain.EventRow{r}}); err != nil {
			a.t.Fatalf("play %s seq %d: %v", id, r.Seq, err)
		}
	}
}

func row(at time.Time, kind domain.EventKind, data string) domain.EventRow {
	return domain.EventRow{At: at, Kind: kind, Data: json.RawMessage(data)}
}

func stateRow(at time.Time, from, to, reason string) domain.EventRow {
	return row(at, domain.KindState, `{"from":"`+from+`","to":"`+to+`","reason":"`+reason+`"}`)
}

func noteRow(at time.Time, text string) domain.EventRow {
	b, _ := json.Marshal(domain.NoteData{Text: text})
	return row(at, domain.KindNote, string(b))
}

var t0 = time.Date(2026, 11, 20, 12, 0, 0, 0, time.UTC)

// seedRun stores a running task of Mia's whose rows have played through set up, with a published version.
func (a *app) seedRun(id string) {
	a.t.Helper()
	task := seeded(id, 0, domain.Waiting, "Mia")
	task.Runner = "build-1"
	a.seed(task)
	a.play(id,
		stateRow(t0, "", "waiting", "started by Mia"),
		stateRow(t0.Add(6*time.Second), "waiting", "starting", "slot taken on build-1"),
		noteRow(t0.Add(10*time.Second), "desk up: build-1"),
		noteRow(t0.Add(20*time.Second), "box up: box:dev"),
		noteRow(t0.Add(25*time.Second), "checked out: main at 5f0e2b1"),
		stateRow(t0.Add(31*time.Second), "starting", "running", "desk, box and proxy up"),
	)
}

// frame is one event of a stream as a client reads it.
type frame struct{ id, typ, data string }

// stream opens a stream on a real server, since the recorder cannot stream. It stops with the test.
func streamOf(t *testing.T, h http.Handler, path string, header map[string]string) <-chan frame {
	t.Helper()
	srv := httptest.NewServer(h)
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(func() { cancel(); srv.Close() })
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, srv.URL+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set(userHeader, "Mia")
	for k, v := range header {
		req.Header.Set(k, v)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != http.StatusOK || !strings.HasPrefix(resp.Header.Get("Content-Type"), "text/event-stream") {
		t.Fatalf("stream %s: %d %s", path, resp.StatusCode, resp.Header.Get("Content-Type"))
	}
	frames := make(chan frame, 64)
	go func() {
		defer close(frames)
		defer func() { _ = resp.Body.Close() }()
		for ev, err := range sse.Read(resp.Body, nil) {
			if err != nil {
				return
			}
			frames <- frame{id: ev.LastEventID, typ: ev.Type, data: ev.Data}
		}
	}()
	return frames
}

func next(t *testing.T, frames <-chan frame) frame {
	t.Helper()
	select {
	case f, ok := <-frames:
		if !ok {
			t.Fatal("the stream ended")
		}
		return f
	case <-time.After(6 * time.Second):
		t.Fatal("no event within 6s")
	}
	return frame{}
}

func quiet(t *testing.T, frames <-chan frame, wait time.Duration) {
	t.Helper()
	select {
	case f := <-frames:
		t.Fatalf("an event came that should not: %+v", f)
	case <-time.After(wait):
	}
}

func seqOf(t *testing.T, f frame) int64 {
	t.Helper()
	var r struct {
		Seq int64 `json:"seq"`
	}
	if err := json.Unmarshal([]byte(f.data), &r); err != nil {
		t.Fatalf("event data %q: %v", f.data, err)
	}
	if f.id != strconv.FormatInt(r.Seq, 10) || f.typ != "row" {
		t.Errorf("event id %q type %q for seq %d", f.id, f.typ, r.Seq)
	}
	return r.Seq
}

func TestARunPageDrawsPartsStepsAndTheSummaryFromTheRows(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	r := a.do(get("Mia", "/tasks/7"))
	r.has(t, http.StatusOK, "Prompt of 7", "#7", "example-org/shop-web", `href="/tasks?from=prompt"`, `aria-label="running"`,
		`aria-label="Run"`, "Summary", "Parts", "Set up", "Agent", "31s", `href="/tasks/7/setup"`, `href="/tasks/7/agent"`,
		"Started by Mia", "from a prompt", "Running", "main at", "5f0e2b1", "Nothing published yet.", "Watch or type in Agent",
		`hx-post="/api/tasks/7/stop"`, `hx-post="/api/tasks/7/end"`, `href="/settings"`,
		`sse-connect="/api/tasks/7/events?after=7&amp;view=page"`)
	r.lacks(t, "Re-run", "Review", "Answer", "Task spec file")
	setup := a.do(get("Mia", "/tasks/7/setup"))
	setup.has(t, http.StatusOK, "Set up", "done in 31s", "Wait for a machine", "Start desk and Box", "Check out example-org/shop-web", "Send the prompt",
		"queued at 12:00:00", "picked by build-1 at 12:00:06", "desk up: build-1", "box up: box:dev", ">main<", "at 5f0e2b1", "typed by Mia", "Prompt of 7", "second line")
}

func TestTheAgentPartShowsTheTerminalAreaNotConnectedWithTheInputOff(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	agent := a.do(get("Mia", "/tasks/7/agent"))
	agent.has(t, http.StatusOK, "running for", "The terminal is not connected yet.", `aria-label="Type to the agent"`, `placeholder="Type to the agent"`, "disabled", "Not connected.",
		`id="term"`, `hx-preserve="true"`, `data-task="7"`, `data-me="Mia"`, `data-readonly="false"`, `src="/static/term.js"`)
	a.do(get("Mia", "/tasks/7/review")).has(t, http.StatusOK, "The task has not reached this part yet.")
	a.do(get("Mia", "/tasks/7/nonsense")).has(t, http.StatusNotFound)
	a.do(get("Mia", "/tasks/99/agent")).has(t, http.StatusNotFound)
}

func TestAFailedTaskShowsItsReasonAndReRun(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	a.play("7", stateRow(t0.Add(3*time.Minute), "running", "failed", "the agent exited with code 1"))
	svc := a.store()
	failed, err := svc.Load(context.Background(), "7")
	if err != nil {
		t.Fatal(err)
	}
	failed.Failure = "the agent exited with code 1"
	failed.EndedAt = t0.Add(3 * time.Minute)
	if _, err := svc.Save(context.Background(), domain.TaskWrite{Task: failed}); err != nil {
		t.Fatal(err)
	}
	r := a.do(get("Mia", "/tasks/7"))
	r.has(t, http.StatusOK, "Agent failed", "View in Agent", "the agent exited with code 1", `hx-post="/api/tasks/7/retry"`, "Re-run", `aria-label="failed"`)
	r.lacks(t, "Nothing published yet.", `hx-post="/api/tasks/7/stop"`, "Result page")
	agent := a.do(get("Mia", "/tasks/7/agent"))
	agent.has(t, http.StatusOK, "failed in 2m 29s", "the agent exited with code 1")
	agent.lacks(t, `aria-label="Type to the agent"`)
}

func TestTheSnapshotHoldsTheTaskItsRowsAndTheLastSeq(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	r := a.do(get("Mia", "/api/tasks/7"))
	r.has(t, http.StatusOK, `"id":"7"`, `"state":"running"`, `"last_seq":7`, `"versions":[]`, `"kind":"state"`, `"runner":"build-1"`)
	var snap struct {
		Events []struct {
			Seq  int64  `json:"seq"`
			Kind string `json:"kind"`
		} `json:"events"`
	}
	if err := json.Unmarshal([]byte(r.body), &snap); err != nil || len(snap.Events) != 7 || snap.Events[6].Seq != 7 {
		t.Errorf("events = %+v, err %v", snap.Events, err)
	}
	a.do(get("Mia", "/api/tasks/99")).has(t, http.StatusNotFound)
	a.do(get("", "/api/tasks/7")).has(t, http.StatusUnauthorized)
}

func TestTheEventStreamResumesAfterADropWithEachRowOnceInOrder(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	first := streamOf(t, a.handler, "/api/tasks/7/events", nil)
	var got []int64
	for range 7 {
		got = append(got, seqOf(t, next(t, first)))
	}
	a.play("7", noteRow(t0.Add(time.Minute), "one more"), noteRow(t0.Add(2*time.Minute), "and another"))
	got = append(got, seqOf(t, next(t, first)), seqOf(t, next(t, first)))
	want := []int64{1, 2, 3, 4, 5, 6, 7, 8, 9}
	if !equalSeqs(got, want) {
		t.Fatalf("first connection saw %v, want %v", got, want)
	}
	a.play("7", noteRow(t0.Add(3*time.Minute), "while no one listens"), noteRow(t0.Add(4*time.Minute), "still none"))
	again := streamOf(t, a.handler, "/api/tasks/7/events", map[string]string{"Last-Event-ID": "9"})
	if s := seqOf(t, next(t, again)); s != 10 {
		t.Errorf("after Last-Event-ID 9 the first row is %d, want 10", s)
	}
	if s := seqOf(t, next(t, again)); s != 11 {
		t.Errorf("then %d, want 11", s)
	}
	quiet(t, again, 1500*time.Millisecond)
	byQuery := streamOf(t, a.handler, "/api/tasks/7/events?after=10", nil)
	if s := seqOf(t, next(t, byQuery)); s != 11 {
		t.Errorf("after=10 starts at %d, want 11", s)
	}
	headerWins := streamOf(t, a.handler, "/api/tasks/7/events?after=0", map[string]string{"Last-Event-ID": "10"})
	if s := seqOf(t, next(t, headerWins)); s != 11 {
		t.Errorf("Last-Event-ID 10 with after=0 starts at %d, want 11", s)
	}
}

func equalSeqs(a, b []int64) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func TestAnEventWrittenThroughOnePortalReachesAStreamOnAnother(t *testing.T) {
	a := newApp(t).withRepos()
	b := a.peer()
	a.seedRun("7")
	rows := streamOf(t, b.handler, "/api/tasks/7/events?after=7", nil)
	page := streamOf(t, b.handler, "/api/tasks/7/events?view=page&after=7", nil)
	if r := a.do(hx(req{method: http.MethodPost, path: "/api/tasks/7/stop", person: "Ken"})); r.code != http.StatusOK {
		t.Fatalf("stop on the first portal: %d %s", r.code, r.body)
	}
	if s := seqOf(t, next(t, rows)); s != 8 {
		t.Errorf("the second portal's stream gave seq %d, want 8", s)
	}
	f := next(t, page)
	if f.typ != "page" || f.id != "8" || !strings.Contains(f.data, `aria-label="cancelled"`) || !strings.Contains(f.data, "Cancelled") {
		t.Errorf("page event = %+v", f)
	}
	if strings.Contains(f.data, "<html") || strings.Contains(f.data, "hx-post") && strings.Contains(f.data, "/stop") {
		t.Errorf("the page event is a whole page or still offers Cancel: %s", f.data)
	}
}

func TestAPageStreamSendsOneDrawingPerBatchWithTheLastIdAndNothingForRowsTheClientHas(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	caughtUp := streamOf(t, a.handler, "/api/tasks/7/events?view=page&after=7&part=setup", nil)
	quiet(t, caughtUp, 1500*time.Millisecond)
	behind := streamOf(t, a.handler, "/api/tasks/7/events?view=page&after=3&part=setup", nil)
	f := next(t, behind)
	if f.id != "7" || f.typ != "page" || !strings.Contains(f.data, "Check out example-org/shop-web") || strings.Contains(f.data, "<!doctype") {
		t.Errorf("catch-up event = id %q type %q: %.200s", f.id, f.typ, f.data)
	}
	a.play("7", noteRow(t0.Add(time.Minute), "one"), noteRow(t0.Add(2*time.Minute), "two"))
	if f := next(t, caughtUp); f.id != "9" {
		t.Errorf("a batch of rows 8 and 9 came as id %q", f.id)
	}
	if f := next(t, behind); f.id != "9" {
		t.Errorf("the batch came as id %q to the second client", f.id)
	}
}

func TestTheStreamRefusesAnUnknownTaskAndABadSeq(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	a.do(get("Mia", "/api/tasks/99/events")).has(t, http.StatusNotFound)
	a.do(get("Mia", "/api/tasks/7/events?after=x")).has(t, http.StatusBadRequest)
	a.do(get("Mia", "/api/tasks/7/events?after=-1")).has(t, http.StatusBadRequest)
	a.do(get("", "/api/tasks/7/events")).has(t, http.StatusUnauthorized)
	a.do(get("", "/api/tasks/events")).has(t, http.StatusUnauthorized)
}

func TestTheTaskListStreamTellsOfAChangeOnAnotherPortalAndOfNothingElse(t *testing.T) {
	a := newApp(t).withRepos()
	b := a.peer()
	page := a.do(get("Mia", "/tasks"))
	stamp := between(t, page.body, `sse-connect="/api/tasks/events?since=`, `"`)
	page.has(t, http.StatusOK, `hx-ext="sse"`, `hx-trigger="change, sse:tasks, task-started from:body"`)
	page.lacks(t, "every 5s")
	changes := streamOf(t, b.handler, "/api/tasks/events?since="+stamp, nil)
	quiet(t, changes, 1500*time.Millisecond)
	a.do(startJSON("Ken", "example-org/shop-web", "Fix links")).has(t, http.StatusCreated)
	f := next(t, changes)
	if f.typ != "tasks" || f.data == "" || f.data == stamp {
		t.Errorf("list event = %+v, stamp was %q", f, stamp)
	}
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/1/stop", person: "Ken"})).has(t, http.StatusOK)
	if f2 := next(t, changes); f2.data == f.data {
		t.Errorf("an update to a listed task gave the same stamp %q", f2.data)
	}
	stale := streamOf(t, b.handler, "/api/tasks/events?since=0.0", nil)
	if f := next(t, stale); f.typ != "tasks" {
		t.Errorf("a stream opened with an old stamp must tell at once, got %+v", f)
	}
}

func TestCancelEndAndReRunAnswerWithToastsAndRefuseWhatTheStateForbids(t *testing.T) {
	a := newApp(t).withRepos()
	a.seedRun("7")
	a.seedRun("8")
	end := a.do(hx(req{method: http.MethodPost, path: "/api/tasks/8/end", person: "Ken"}))
	end.has(t, http.StatusOK, "Ended. The task is done.", `id="toast"`)
	if end.header.Get("HX-Reswap") != "none" {
		t.Errorf("HX-Reswap = %q", end.header.Get("HX-Reswap"))
	}
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/8/end", person: "Ken"})).has(t, http.StatusOK, "Ended.")
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/8/stop", person: "Ken"})).has(t, http.StatusConflict, "The task has already finished.")
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/7/retry", person: "Ken"})).has(t, http.StatusConflict, "Only a failed task that a person started can be re-run.")
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/7/stop", person: "Ken"})).has(t, http.StatusOK, "Cancelled.")
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/7/stop", person: "Ken"})).has(t, http.StatusOK, "Cancelled.")
	a.do(hx(req{method: http.MethodPost, path: "/api/tasks/99/stop", person: "Ken"})).has(t, http.StatusNotFound)
	for _, path := range []string{"/api/tasks/7/stop", "/api/tasks/7/end", "/api/tasks/7/retry"} {
		a.do(req{method: http.MethodPost, path: path}).has(t, http.StatusUnauthorized)
		r := req{method: http.MethodPost, path: path, person: "Ken", headers: map[string]string{"Sec-Fetch-Site": "cross-site"}}
		a.do(r).has(t, http.StatusForbidden)
	}
	if got := a.list("Mia", "?state=cancelled"); len(got.Tasks) != 1 || got.Tasks[0].ID != "7" {
		t.Errorf("cancelled = %+v", got.Tasks)
	}
}

func TestReRunStartsANewTaskWithTheSameInputsAndLinksIt(t *testing.T) {
	a := newApp(t).withRepos()
	a.do(startJSON("Mia", "example-org/shop-web", "Upgrade the date library")).has(t, http.StatusCreated)
	svc := a.service()
	ctx := context.Background()
	if _, err := svc.Report(ctx, "1", task.Report{Kind: task.SlotTaken, Runner: "build-1"}); err != nil {
		t.Fatal(err)
	}
	if _, err := svc.Report(ctx, "1", task.Report{Kind: task.PartFailed, Run: 1, Text: "the Box image was not found"}); err != nil {
		t.Fatal(err)
	}
	page := a.do(get("Mia", "/tasks/1"))
	page.has(t, http.StatusOK, "Set up failed", "View in Set up", "the Box image was not found", "Re-run")
	r := a.do(hx(req{method: http.MethodPost, path: "/api/tasks/1/retry", person: "Ken"}))
	r.has(t, http.StatusOK, "Started task #2 with the same prompt and repository.")
	again := a.do(get("Mia", "/api/tasks/2"))
	again.has(t, http.StatusOK, `"prompt":"Upgrade the date library"`, `"started_by":"Mia"`, `"state":"waiting"`, `retry of 1`)
	j := a.do(req{method: http.MethodPost, path: "/api/tasks/1/retry", person: "Ken", json: true, body: "{}"})
	j.has(t, http.StatusOK, `"id":"3"`)
}
