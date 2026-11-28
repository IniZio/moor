package herdrtest_test

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/herdr"
	"github.com/IniZio/moor/internal/adapter/herdr/herdrtest"
)

type recorded struct {
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
	Result json.RawMessage `json:"result"`
	Error  *struct {
		Kind string `json:"kind"`
	} `json:"error"`
}

type view struct {
	Kind      string
	Address   string
	Labels    map[string]string
	State     string
	Status    string
	ExitCode  *int
	Signal    *int
	Terminals int
}

func viewOf(method string, result json.RawMessage, kind string) view {
	if kind != "" {
		return view{Kind: kind}
	}
	switch method {
	case "terminal.list":
		var l struct{ Terminals []herdr.Terminal }
		_ = json.Unmarshal(result, &l)
		return view{Terminals: len(l.Terminals)}
	case "terminal.run", "terminal.get", "terminal.kill":
		var t herdr.Terminal
		_ = json.Unmarshal(result, &t)
		v := view{Address: t.Address, Labels: t.Labels, State: t.State, ExitCode: t.ExitCode, Signal: t.Signal}
		if t.Status != nil {
			v.Status = t.Status.State + "/" + t.Status.Kind
		}
		return v
	}
	return view{}
}

var acts = map[int]func(c *herdrtest.Core){
	3: func(c *herdrtest.Core) { c.SetStatus("agent-r1", "working", "") },
	5: func(c *herdrtest.Core) { c.SetStatus("agent-r1", "blocked", "") },
	8: func(c *herdrtest.Core) { c.Exit("agent-r1", 3) },
}

func TestFakeAnswersAsTheRealTranscript(t *testing.T) {
	b, err := os.ReadFile("../testdata/transcript.jsonl")
	if err != nil {
		t.Fatal(err)
	}
	t.Log("transcript recorded against herdr-core 0.1.16; re-record against the pinned version with make herdr-golden")
	core := herdrtest.New()
	c := herdr.Dial(core.Connect())
	defer func() { _ = c.Close() }()
	for i, line := range strings.Split(strings.TrimSpace(string(b)), "\n") {
		var r recorded
		_ = json.Unmarshal([]byte(line), &r)
		if act := acts[i]; act != nil {
			act(core)
		}
		if r.Method == "machine.exec" || r.Method == "hello" {
			continue
		}
		if got, exp := ask(t, c, r), expected(r); !reflect.DeepEqual(got, exp) {
			t.Errorf("call %d %s: fake %+v, real %+v", i, r.Method, got, exp)
		}
	}
}

func ask(t *testing.T, c *herdr.Conn, r recorded) view {
	x, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var params any
	_ = json.Unmarshal(r.Params, &params)
	var res json.RawMessage
	err := c.Call(x, r.Method, params, &res)
	var he *herdr.Error
	if err != nil && !errors.As(err, &he) {
		t.Fatalf("%s: %v", r.Method, err)
	}
	kind := ""
	if he != nil {
		kind = he.Kind
	}
	v := viewOf(r.Method, res, kind)
	v.Status = trimKind(v.Status)
	return v
}

func expected(r recorded) view {
	kind := ""
	if r.Error != nil {
		kind = r.Error.Kind
	}
	v := viewOf(r.Method, r.Result, kind)
	v.Status = trimKind(v.Status)
	return v
}

func trimKind(s string) string { return strings.TrimSuffix(strings.TrimSuffix(s, "question"), "/") }

func TestFakeEventsFollowTheRealOrder(t *testing.T) {
	core := herdrtest.New()
	var mu sync.Mutex
	var types []string
	x, cancel := context.WithCancel(context.Background())
	snap := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		_ = herdr.Subscribe(x, core.Connect(), map[string]string{"moor.task": "7"},
			func([]herdr.Terminal) { close(snap) },
			func(e herdr.Event) { mu.Lock(); types = append(types, e.Terminal+":"+e.Type); mu.Unlock() })
	}()
	<-snap
	c := herdr.Dial(core.Connect())
	if _, err := c.Run(x, herdr.RunReq{Name: "agent-r1", Labels: map[string]string{"moor.task": "7"}}); err != nil {
		t.Fatal(err)
	}
	core.SetStatus("agent-r1", "working", "")
	core.SetStatus("agent-r1", "blocked", "")
	core.Exit("agent-r1", 3)
	if _, err := c.Run(x, herdr.RunReq{Name: "agent-r1", Labels: map[string]string{"moor.task": "7"}}); err != nil {
		t.Fatal(err)
	}
	if err := c.Kill(x, "agent-r1"); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Run(x, herdr.RunReq{Name: "other"}); err != nil {
		t.Fatal(err)
	}
	want := "t1:created t1:status t1:status t1:status t1:finished t2:created t2:finished"
	deadline := time.Now().Add(5 * time.Second)
	for {
		mu.Lock()
		got := strings.Join(types, " ")
		mu.Unlock()
		if got == want {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("events %q, want the real order less foreground and turn: %q", got, want)
		}
		time.Sleep(time.Millisecond)
	}
	cancel()
	<-done
}

func TestFakeRecordsSendsAndInterrupts(t *testing.T) {
	core := herdrtest.New()
	c := herdr.Dial(core.Connect())
	x := context.Background()
	if err := c.Hello(x); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Run(x, herdr.RunReq{Name: "a"}); err != nil {
		t.Fatal(err)
	}
	core.SetStatus("a", "blocked", "")
	if err := c.Send(x, "a", "hi"); err != nil {
		t.Fatal(err)
	}
	if err := c.Interrupt(x, "a"); err != nil {
		t.Fatal(err)
	}
	if got := core.Sent("a"); len(got) != 1 || got[0] != "hi" || core.Interrupts("a") != 1 {
		t.Fatalf("sent %v, interrupts %d", got, core.Interrupts("a"))
	}
	if core.Sent("nosuch") != nil || core.Interrupts("nosuch") != 0 || core.Connections() != 1 {
		t.Fatal("unknown terminals have no record")
	}
}

func TestFakeExecsSignalsAndStops(t *testing.T) {
	core := herdrtest.New()
	core.SetExec(func(_ context.Context, r herdr.ExecReq) herdr.ExecResult {
		return herdrtest.Exit(4, strings.Join(r.Argv, " "))
	})
	c := herdr.Dial(core.Connect())
	x := context.Background()
	r, err := c.Exec(x, herdr.ExecReq{Argv: []string{"x", "y"}})
	if err != nil || *r.Code != 4 || string(r.Stdout) != "x y" {
		t.Fatalf("exec %+v %v", r, err)
	}
	if _, err := c.Run(x, herdr.RunReq{Name: "a"}); err != nil {
		t.Fatal(err)
	}
	core.Signal("a", 15)
	if ts := core.Terminals(); len(ts) != 1 || ts[0].Code() != 143 {
		t.Fatalf("a signal death is 128+signal: %+v", ts)
	}
	core.Stop()
	if _, err := c.Get(x, "a"); err == nil {
		t.Fatal("a stopped core answers nothing")
	}
}
