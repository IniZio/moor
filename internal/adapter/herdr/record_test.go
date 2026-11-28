//go:build herdrgolden

package herdr_test

import (
	"bufio"
	"encoding/json"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestRecordTranscripts(t *testing.T) {
	dir, err := os.MkdirTemp("", "hcg")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	env := append(os.Environ(), "HC_SESSION=g", "XDG_STATE_HOME="+dir+"/s", "XDG_RUNTIME_DIR="+dir+"/r", "XDG_CONFIG_HOME="+dir+"/c")
	t.Cleanup(func() {
		stop := exec.Command("hc", "server", "stop")
		stop.Env = env
		_ = stop.Run()
	})
	events := &recorder{}
	sub := connect(t, env)
	go events.follow(sub)
	sub.send(t, "events.subscribe", map[string]any{"labels": map[string]string{"moor.task": "7"}})

	main := &recorder{}
	cmd := connect(t, env)
	for _, s := range scenario {
		if s.method == "" {
			time.Sleep(s.wait)
			continue
		}
		cmd.send(t, s.method, s.params)
		main.add(t, s.method, s.params, cmd.next(t))
	}
	time.Sleep(300 * time.Millisecond)
	main.write(t, "transcript.jsonl")
	events.write(t, "events.jsonl")
}

type step struct {
	method string
	params any
	wait   time.Duration
}

func pause(d time.Duration) step { return step{wait: d} }

var (
	labels1 = map[string]string{"moor.task": "7", "moor.run": "1"}
	labels2 = map[string]string{"moor.task": "7", "moor.run": "2"}
	agent   = "sleep 0.3; printf '\\033]7501;state=working\\007'; read x; " +
		"printf '\\033]7501;state=blocked:kind=question\\007'; read y; exit 3"
)

var scenario = []step{
	{method: "hello", params: map[string]any{}},
	{method: "terminal.run", params: map[string]any{
		"argv": []string{"sh", "-c", agent}, "name": "agent-r1", "cwd": "/", "labels": labels1,
		"env": map[string]string{"A": "b"}, "no_parent": true, "outlive_parent": true, "no_notify": true,
	}},
	{method: "terminal.run", params: map[string]any{"argv": []string{"sleep", "9"}, "name": "agent-r1", "cwd": "/"}},
	pause(700 * time.Millisecond),
	{method: "terminal.list", params: map[string]any{"all": true, "labels": map[string]string{"moor.task": "7"}}},
	{method: "terminal.send", params: map[string]any{"terminal": "agent-r1", "text": "one", "force": true}},
	pause(500 * time.Millisecond),
	{method: "terminal.get", params: map[string]any{"terminal": "agent-r1"}},
	{method: "terminal.send", params: map[string]any{"terminal": "agent-r1", "text": "no"}},
	{method: "terminal.send", params: map[string]any{"terminal": "agent-r1", "text": "two", "force": true}},
	pause(500 * time.Millisecond),
	{method: "terminal.get", params: map[string]any{"terminal": "agent-r1"}},
	{method: "terminal.send", params: map[string]any{"terminal": "agent-r1", "text": "late", "force": true}},
	{method: "terminal.run", params: map[string]any{
		"argv": []string{"sleep", "30"}, "name": "agent-r1", "cwd": "/", "labels": labels2, "no_parent": true,
	}},
	pause(300 * time.Millisecond),
	{method: "terminal.kill", params: map[string]any{"terminal": "agent-r1"}},
	{method: "terminal.interrupt", params: map[string]any{"terminal": "nosuch", "timeout": 1}},
	{method: "machine.exec", params: map[string]any{"argv": []string{"sh", "-c", "echo out; exit 2"}, "timeout": 10}},
	{method: "terminal.read", params: map[string]any{"terminal": "agent-r1", "lines": 0}},
	{method: "terminal.read", params: map[string]any{"terminal": "nosuch", "lines": 0}},
}

type line struct {
	r  *bufio.Reader
	w  io.Writer
	id int
}

func connect(t *testing.T, env []string) *line {
	c := exec.Command("hc", "connect")
	c.Env, c.Dir = env, "/"
	in, err := c.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	out, err := c.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = in.Close(); _ = c.Wait() })
	return &line{r: bufio.NewReaderSize(out, 1<<20), w: in}
}

func (l *line) send(t *testing.T, method string, params any) {
	l.id++
	b, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": l.id, "method": method, "params": params})
	if _, err := l.w.Write(append(b, '\n')); err != nil {
		t.Fatal(err)
	}
}

func (l *line) next(t *testing.T) map[string]any {
	for {
		s, err := l.r.ReadString('\n')
		if err != nil {
			t.Fatal(err)
		}
		var m map[string]any
		if json.Unmarshal([]byte(s), &m) == nil && m["id"] != nil {
			return m
		}
	}
}

type recorder struct {
	mu    sync.Mutex
	lines []any
}

func (r *recorder) add(t *testing.T, method string, params any, reply map[string]any) {
	entry := map[string]any{"method": method, "params": params}
	if e, ok := reply["error"].(map[string]any); ok {
		data, _ := e["data"].(map[string]any)
		entry["error"] = map[string]any{"kind": data["kind"]}
	} else {
		entry["result"] = scrub(reply["result"])
	}
	r.mu.Lock()
	r.lines = append(r.lines, entry)
	r.mu.Unlock()
}

func (r *recorder) follow(l *line) {
	for {
		s, err := l.r.ReadString('\n')
		if err != nil {
			return
		}
		var m map[string]any
		if json.Unmarshal([]byte(s), &m) != nil {
			continue
		}
		r.mu.Lock()
		r.lines = append(r.lines, scrub(m))
		r.mu.Unlock()
	}
}

func (r *recorder) write(t *testing.T, name string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	var sb strings.Builder
	for _, l := range r.lines {
		b, _ := json.Marshal(l)
		sb.Write(append(b, '\n'))
	}
	if err := os.WriteFile(filepath.Join("testdata", name), []byte(sb.String()), 0o644); err != nil {
		t.Fatal(err)
	}
}

var dropped = map[string]bool{
	"pid": true, "created_ms": true, "finished_ms": true, "status_ms": true, "ms": true, "core": true,
	"foreground": true, "cwd": true, "identity": true, "methods": true,
}

func scrub(v any) any {
	switch x := v.(type) {
	case map[string]any:
		out := map[string]any{}
		for k, val := range x {
			if !dropped[k] {
				out[k] = scrub(val)
			}
		}
		return out
	case []any:
		for i := range x {
			x[i] = scrub(x[i])
		}
	}
	return v
}
