// Package herdrtest is a fake herdr-core: hc's JSON-RPC over in-memory pipes, with terminals, labels,
// statuses, finished terminals, name_taken, blocked sends and event subscriptions. The transcripts
// in ../testdata keep it honest.
package herdrtest

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"maps"
	"net"
	"slices"
	"strconv"
	"sync"

	"github.com/IniZio/moor/internal/adapter/herdr"
)

type Core struct {
	mu     sync.Mutex
	terms  []*term
	seq    uint64
	conns  map[*conn]bool
	exec   func(context.Context, herdr.ExecReq) herdr.ExecResult
	nconns int
}

type term struct {
	t     herdr.Terminal
	sent  []string
	intrs int
}

type conn struct {
	core  *Core
	pipe  net.Conn
	sub   map[string]string
	queue chan []byte
	done  chan struct{}
	ctx   context.Context
	stop  context.CancelFunc
	once  sync.Once
}

func New() *Core {
	return &Core{conns: map[*conn]bool{}, exec: func(context.Context, herdr.ExecReq) herdr.ExecResult { return Exit(0, "") }}
}

func Exit(code int, stdout string) herdr.ExecResult {
	return herdr.ExecResult{Code: &code, Stdout: []byte(stdout), Stderr: []byte{}}
}

func (c *Core) SetExec(fn func(context.Context, herdr.ExecReq) herdr.ExecResult) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.exec = fn
}

func (c *Core) Connect() io.ReadWriteCloser {
	ours, theirs := net.Pipe()
	ctx, stop := context.WithCancel(context.Background())
	k := &conn{core: c, pipe: ours, queue: make(chan []byte, 256), done: make(chan struct{}), ctx: ctx, stop: stop}
	c.mu.Lock()
	c.conns[k] = true
	c.nconns++
	c.mu.Unlock()
	go k.serve()
	return theirs
}

func (c *Core) Connections() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.nconns
}

func (c *Core) Stop() {
	c.mu.Lock()
	conns := slices.Collect(maps.Keys(c.conns))
	c.mu.Unlock()
	for _, k := range conns {
		k.close()
	}
}

func (c *Core) Terminals() []herdr.Terminal {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]herdr.Terminal, len(c.terms))
	for i, t := range c.terms {
		out[i] = t.t
	}
	return out
}

func (c *Core) Sent(ref string) []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	if t := c.find(ref); t != nil {
		return slices.Clone(t.sent)
	}
	return nil
}

func (c *Core) Interrupts(ref string) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	if t := c.find(ref); t != nil {
		return t.intrs
	}
	return 0
}

func (c *Core) SetStatus(ref, state, msg string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	t := c.find(ref)
	if t == nil {
		return
	}
	t.t.Status = nil
	if state != "" {
		t.t.Status = &herdr.Status{State: state, Msg: msg}
	}
	c.emit(t, "status", map[string]any{"status": t.t.Status, "reported": true})
}

func (c *Core) Exit(ref string, code int) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if t := c.find(ref); t != nil && t.t.Live() {
		c.finish(t, &code, nil, "exited")
	}
}

func (c *Core) Signal(ref string, sig int) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if t := c.find(ref); t != nil && t.t.Live() {
		c.finish(t, nil, &sig, "exited")
	}
}

func (c *Core) finish(t *term, code, sig *int, reason string) {
	t.t.State, t.t.ExitCode, t.t.Signal, t.t.Reason = "finished", code, sig, &reason
	if t.t.Status != nil {
		t.t.Status = nil
		c.emit(t, "status", map[string]any{"status": nil, "reported": true})
	}
	c.emit(t, "finished", map[string]any{"exit_code": code, "signal": sig, "reason": reason})
}

func (c *Core) find(ref string) *term {
	for i := len(c.terms) - 1; i >= 0; i-- {
		t := c.terms[i]
		if t.t.Address == ref || (t.t.Name != nil && *t.t.Name == ref) {
			return t
		}
	}
	return nil
}

func (c *Core) emit(t *term, typ string, data any) {
	c.seq++
	ev := map[string]any{"seq": c.seq, "terminal": t.t.Address, "type": typ, "data": data}
	b, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "method": "event", "params": ev})
	for k := range c.conns {
		if k.sub != nil && matches(t.t.Labels, k.sub) {
			k.enqueue(b)
		}
	}
}

func matches(have, want map[string]string) bool {
	for k, v := range want {
		if have[k] != v {
			return false
		}
	}
	return true
}

func (c *Core) addTerm(r herdr.RunReq) (herdr.Terminal, *rpcError) {
	if r.Name != "" {
		if t := c.find(r.Name); t != nil && t.t.Live() {
			return herdr.Terminal{}, &rpcError{Kind: herdr.KindNameTaken, Message: fmt.Sprintf("a live terminal is already named %q", r.Name)}
		}
	}
	t := &term{t: herdr.Terminal{
		Address: "t" + strconv.Itoa(len(c.terms)+1), Labels: maps.Clone(r.Labels), State: "live",
	}}
	if t.t.Labels == nil {
		t.t.Labels = map[string]string{}
	}
	if r.Name != "" {
		t.t.Name = &r.Name
	}
	c.terms = append(c.terms, t)
	c.emit(t, "created", t.t)
	return t.t, nil
}
