package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"slices"
	"testing"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
)

type seatNotice struct {
	Seat     string   `json:"seat"`
	Holder   string   `json:"holder"`
	Watchers []string `json:"watchers"`
	Ended    string   `json:"ended"`
}

// runningTask starts a task on a machine linked to p and waits for it to run.
func runningTask(t *testing.T, p *replica) (domain.TaskID, *machineRig) {
	t.Helper()
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	id := p.startTask("Fix the checkout")
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	return id, m
}

func dialTerm(t *testing.T, addr string, id domain.TaskID, person, mode string) *websocket.Conn {
	t.Helper()
	c, resp, err := dialTermAs(addr, id, person, mode, "http://"+addr)
	if err != nil {
		t.Fatalf("dial the terminal as %s: %v %v", person, err, resp)
	}
	t.Cleanup(func() { _ = c.CloseNow() })
	return c
}

func dialTermAs(addr string, id domain.TaskID, person, mode, origin string) (*websocket.Conn, *http.Response, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	h := http.Header{userHeader: {person}, "Origin": {origin}}
	c, resp, err := websocket.Dial(ctx, "ws://"+addr+"/api/tasks/"+string(id)+"/terminal?mode="+mode+"&cols=100&rows=30", &websocket.DialOptions{HTTPHeader: h})
	if err != nil {
		return nil, resp, fmt.Errorf("dial: %w", err)
	}
	return c, resp, nil
}

// next reads the connection's next message of the kind and skips the others.
func nextMsg(t *testing.T, c *websocket.Conn, want websocket.MessageType) []byte {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	for {
		typ, b, err := c.Read(ctx)
		if err != nil {
			t.Fatalf("read: %v", err)
		}
		if typ == want {
			return b
		}
	}
}

func notice(t *testing.T, c *websocket.Conn) seatNotice {
	t.Helper()
	var n seatNotice
	if err := json.Unmarshal(nextMsg(t, c, websocket.MessageText), &n); err != nil {
		t.Fatal(err)
	}
	return n
}

func noticeUntil(t *testing.T, c *websocket.Conn, ok func(seatNotice) bool) seatNotice {
	t.Helper()
	for {
		if n := notice(t, c); ok(n) {
			return n
		}
	}
}

// closedWith reads until the server closes the connection and returns the close code.
func closedWith(t *testing.T, c *websocket.Conn) websocket.StatusCode {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	for {
		if _, _, err := c.Read(ctx); err != nil {
			return websocket.CloseStatus(err)
		}
	}
}

func typeKeys(t *testing.T, c *websocket.Conn, keys string) {
	t.Helper()
	if err := c.Write(context.Background(), websocket.MessageBinary, []byte(keys)); err != nil {
		t.Fatal(err)
	}
}

func inputAs(person string, id domain.TaskID, text string) req {
	b, _ := json.Marshal(map[string]string{"text": text})
	return req{method: http.MethodPost, path: "/api/tasks/" + string(id) + "/input", person: person, body: string(b), json: true}
}

// anaWatches opens a watcher whose keys go nowhere, and checks both sides are told of it.
func anaWatches(t *testing.T, p *replica, id domain.TaskID, holder *websocket.Conn) {
	t.Helper()
	ana := dialTerm(t, p.addr, id, "Ana", "watch")
	if n := notice(t, ana); n.Seat != "watch" || n.Holder != "Mia" || !slices.Equal(n.Watchers, []string{"Ana"}) {
		t.Fatalf("Ana's first notice %+v", n)
	}
	noticeUntil(t, holder, func(n seatNotice) bool { return slices.Equal(n.Watchers, []string{"Ana"}) })
	typeKeys(t, ana, "rm -rf /\r")
}

func TestTheSeatHolderTypesAWatcherOnlyWatchesAndTakingTheSeatIsExplicit(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	id, m := runningTask(t, p)
	mia := dialTerm(t, p.addr, id, "Mia", "steer")
	if n := notice(t, mia); n.Seat != "steer" || n.Holder != "Mia" {
		t.Fatalf("Mia's first notice %+v", n)
	}
	typeKeys(t, mia, "ls\r")
	if got := string(nextMsg(t, mia, websocket.MessageBinary)); got != "echo:ls\r" {
		t.Fatalf("Mia's terminal printed %q", got)
	}
	anaWatches(t, p, id, mia)
	r := inputAs("Ana", id, "from the line")
	p.do(r).has(t, http.StatusConflict, "Mia has the keyboard.")
	if keys := m.sessions.keysOf("observe"); keys != "" {
		t.Fatalf("a watcher's keys reached the session: %q", keys)
	}

	taker := dialTerm(t, p.addr, id, "Ana", "steer")
	noticeUntil(t, taker, func(n seatNotice) bool { return n.Seat == "steer" && n.Holder == "Ana" })
	if code := closedWith(t, mia); code != 4002 {
		t.Fatalf("Mia's connection ended with %d, want 4002 for a taken seat", code)
	}
	r.person = "Ana"
	p.do(r).has(t, http.StatusNoContent)
	p.do(get("Ana", "/api/tasks/"+string(id)+"/screen")).has(t, http.StatusOK, "from the line")
	if !m.host.holds(string(id)) {
		t.Fatal("the session went with its viewers")
	}
}

func TestATerminalOnAnotherReplicaHopsToTheOneHoldingTheLink(t *testing.T) {
	dsn := pgtest.SchemaDSN(t)
	p := startReplica(t, dsn, "127.0.0.1:0")
	other := startReplica(t, dsn, "127.0.0.1:0")
	id, m := runningTask(t, p)
	c := dialTerm(t, other.addr, id, "Mia", "steer")
	if n := notice(t, c); n.Holder != "Mia" {
		t.Fatalf("notice through the hop %+v", n)
	}
	typeKeys(t, c, "make test\r")
	if got := string(nextMsg(t, c, websocket.MessageBinary)); got != "echo:make test\r" {
		t.Fatalf("through the hop the terminal printed %q", got)
	}
	_ = c.Close(websocket.StatusNormalClosure, "")
	r := inputAs("Mia", id, "add a test")
	other.do(r).has(t, http.StatusNoContent)
	other.do(get("Ana", "/api/tasks/"+string(id)+"/screen")).has(t, http.StatusOK, "add a test")
	if keys := m.sessions.keysOf("write"); keys != "make test\r" {
		t.Fatalf("the session got %q", keys)
	}

	forged := get("Mia", "/internal/terminal/"+string(id)+"/screen")
	forged.headers = map[string]string{"X-Moor-Person": "Mia", "Authorization": "Bearer guess"}
	p.do(forged).has(t, http.StatusUnauthorized)
}

func TestACrossSiteTerminalIsRefusedAndATaskWithoutASessionSaysSo(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	p.withRepos()
	id := p.startTask("Fix the checkout")
	_, resp, err := dialTermAs(p.addr, id, "Mia", "steer", "https://evil.example.com")
	if err == nil || resp == nil || resp.StatusCode != http.StatusForbidden {
		t.Fatalf("a cross-site terminal got %v %v", err, resp)
	}
	if _, resp, err := dialTermAs(p.addr, id, "", "steer", "http://"+p.addr); err == nil || resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("a terminal without a person got %v %v", err, resp)
	}
	c := dialTerm(t, p.addr, id, "Mia", "steer")
	if n := notice(t, c); n.Ended != "the task has no live session" {
		t.Fatalf("a waiting task's terminal said %+v", n)
	}
	p.do(get("Mia", "/api/tasks/"+string(id)+"/screen")).has(t, http.StatusConflict, "no live session")
}

func TestACancelReachesTheRunnerWithoutWaitingForTheNextTick(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0", func(c *config) { c.linkTick = time.Hour })
	p.withRepos()
	_, db := p.tasks()
	id := p.startTask("Fix the checkout")
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	stop := req{method: http.MethodPost, path: "/api/tasks/" + string(id) + "/stop", person: "Mia", headers: map[string]string{"Sec-Fetch-Site": "same-origin"}}
	p.do(stop).has(t, http.StatusOK)
	eventually(t, "the runner removed the cancelled task", func() bool { return !m.host.holds(string(id)) })
}
