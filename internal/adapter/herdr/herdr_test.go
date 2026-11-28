package herdr_test

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"net"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/herdr"
)

type entry struct {
	Method string          `json:"method"`
	Result json.RawMessage `json:"result"`
	Error  *struct {
		Kind string `json:"kind"`
	} `json:"error"`
}

func loadLines(t *testing.T, name string) []string {
	t.Helper()
	b, err := os.ReadFile("testdata/" + name)
	if err != nil {
		t.Fatal(err)
	}
	t.Log("transcript recorded against herdr-core 0.1.16; re-record against the pinned version with make herdr-golden")
	return strings.Split(strings.TrimSpace(string(b)), "\n")
}

func replay(t *testing.T, lines []string) *herdr.Conn {
	t.Helper()
	ours, theirs := net.Pipe()
	t.Cleanup(func() { _ = ours.Close() })
	go func() {
		sc := bufio.NewScanner(ours)
		for i := 0; sc.Scan() && i < len(lines); i++ {
			if _, err := ours.Write(answer(t, i, sc.Bytes(), lines[i])); err != nil {
				return
			}
		}
	}()
	c := herdr.Dial(theirs)
	t.Cleanup(func() { _ = c.Close() })
	return c
}

func answer(t *testing.T, i int, request []byte, recorded string) []byte {
	var req struct {
		ID     uint64 `json:"id"`
		Method string `json:"method"`
	}
	var e entry
	_ = json.Unmarshal(request, &req)
	_ = json.Unmarshal([]byte(recorded), &e)
	if req.Method != e.Method {
		t.Errorf("call %d is %s, the transcript has %s", i, req.Method, e.Method)
	}
	msg := map[string]any{"jsonrpc": "2.0", "id": req.ID, "result": e.Result}
	if e.Error != nil {
		msg = map[string]any{"jsonrpc": "2.0", "id": req.ID, "error": map[string]any{
			"code": -32000, "message": "recorded", "data": map[string]string{"kind": e.Error.Kind},
		}}
	}
	b, _ := json.Marshal(msg)
	return append(b, '\n')
}

func ctx(t *testing.T) context.Context {
	t.Helper()
	c, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	return c
}

func TestClientReadsTheRealTranscript(t *testing.T) {
	c := replay(t, loadLines(t, "transcript.jsonl"))
	x := ctx(t)
	labels := map[string]string{"moor.task": "7", "moor.run": "1"}
	if err := c.Hello(x); err != nil {
		t.Fatal(err)
	}
	t1, err := c.Run(x, herdr.RunReq{Argv: []string{"sh"}, Name: "agent-r1", Labels: labels})
	if err != nil || t1.Address != "t1" || !t1.Live() || t1.Status != nil {
		t.Fatalf("run: %+v %v", t1, err)
	}
	if _, err := c.Run(x, herdr.RunReq{Argv: []string{"sleep"}, Name: "agent-r1"}); !herdr.IsKind(err, herdr.KindNameTaken) {
		t.Fatalf("a live name is taken: %v", err)
	}
	ts, err := c.List(x, map[string]string{"moor.task": "7"})
	if err != nil || len(ts) != 1 || ts[0].Labels["moor.run"] != "1" || ts[0].Status.State != "working" {
		t.Fatalf("list: %+v %v", ts, err)
	}
	if err := c.Send(x, "agent-r1", "one"); err != nil {
		t.Fatal(err)
	}
	got, err := c.Get(x, "agent-r1")
	if err != nil || got.Status.State != "blocked" || got.Status.Kind != "question" {
		t.Fatalf("blocked: %+v %v", got, err)
	}
	if err := c.Call(x, "terminal.send", map[string]any{"terminal": "agent-r1", "text": "no"}, nil); !herdr.IsKind(err, "blocked") {
		t.Fatalf("a blocked program refuses a send without force: %v", err)
	}
	clientReadsTheEnd(t, c)
}

func clientReadsTheEnd(t *testing.T, c *herdr.Conn) {
	x := ctx(t)
	if err := c.Send(x, "agent-r1", "two"); err != nil {
		t.Fatalf("Send forces past blocked: %v", err)
	}
	done, err := c.Get(x, "agent-r1")
	if err != nil || done.Live() || done.Code() != 3 || done.Status != nil {
		t.Fatalf("finished: %+v %v", done, err)
	}
	if err := c.Send(x, "agent-r1", "late"); !herdr.IsKind(err, "finished") {
		t.Fatalf("a finished program takes no text: %v", err)
	}
	t2, err := c.Run(x, herdr.RunReq{Argv: []string{"sleep"}, Name: "agent-r1"})
	if err != nil || t2.Address != "t2" {
		t.Fatalf("a finished name is reusable: %+v %v", t2, err)
	}
	if err := c.Kill(x, "agent-r1"); err != nil {
		t.Fatal(err)
	}
	if err := c.Interrupt(x, "nosuch"); !herdr.IsKind(err, herdr.KindNotFound) {
		t.Fatalf("interrupt of nothing: %v", err)
	}
	r, err := c.Exec(x, herdr.ExecReq{Argv: []string{"sh"}})
	if err != nil || r.OK() || *r.Code != 2 || string(r.Stdout) != "out\n" {
		t.Fatalf("exec: %+v %v", r, err)
	}
}

func TestKilledTerminalReportsItsSignal(t *testing.T) {
	var killed herdr.Terminal
	lines := loadLines(t, "transcript.jsonl")
	for _, l := range lines {
		var e entry
		_ = json.Unmarshal([]byte(l), &e)
		if e.Method == "terminal.kill" {
			_ = json.Unmarshal(e.Result, &killed)
		}
	}
	if killed.Live() || killed.Code() != 129 || *killed.Reason != "killed" {
		t.Fatalf("hc kills with SIGHUP, so the code is 129: %+v", killed)
	}
	if (herdr.Terminal{}).Code() != -1 {
		t.Fatal("no code and no signal is -1")
	}
}

func TestSubscribeReadsRealEvents(t *testing.T) {
	ours, theirs := net.Pipe()
	lines := loadLines(t, "events.jsonl")
	go func() {
		r := bufio.NewReader(ours)
		_, _ = r.ReadString('\n')
		for _, l := range lines {
			_, _ = ours.Write([]byte(l + "\n"))
		}
		_ = ours.Close()
	}()
	var snaps int
	var types []string
	err := herdr.Subscribe(ctx(t), theirs, map[string]string{"moor.task": "7"},
		func(ts []herdr.Terminal) { snaps++ },
		func(e herdr.Event) { types = append(types, e.Terminal+":"+e.Type) })
	if !errors.Is(err, herdr.ErrClosed) {
		t.Fatalf("the feed ends with the stream: %v", err)
	}
	want := "t1:created t1:status t1:foreground t1:status t1:turn t1:status t1:finished t2:created t2:foreground t2:finished"
	if snaps != 1 || strings.Join(types, " ") != want {
		t.Fatalf("snapshots %d, events %v", snaps, types)
	}
}

func TestSubscribeEndsWithItsContext(t *testing.T) {
	_, theirs := net.Pipe()
	x, cancel := context.WithCancel(context.Background())
	go func() { time.Sleep(10 * time.Millisecond); cancel() }()
	err := herdr.Subscribe(x, theirs, nil, func([]herdr.Terminal) {}, func(herdr.Event) {})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v", err)
	}
}

func TestCallsFailOnceTheStreamEnds(t *testing.T) {
	ours, theirs := net.Pipe()
	c := herdr.Dial(theirs)
	go func() { _, _ = bufio.NewReader(ours).ReadString('\n'); _ = ours.Close() }()
	if _, err := c.Get(ctx(t), "t1"); !errors.Is(err, herdr.ErrClosed) {
		t.Fatalf("got %v", err)
	}
	x, cancel := context.WithCancel(context.Background())
	cancel()
	o2, t2 := net.Pipe()
	defer func() { _ = o2.Close() }()
	go func() { _, _ = bufio.NewReader(o2).ReadString('\n') }()
	if err := herdr.Dial(t2).Kill(x, "t1"); !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v", err)
	}
}
