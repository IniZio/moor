package herdrtest

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/IniZio/moor/internal/adapter/herdr"
)

type request struct {
	ID     uint64          `json:"id"`
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
}

type rpcError struct {
	Kind    string
	Message string
}

type target struct {
	Terminal string `json:"terminal"`
	Force    bool   `json:"force"`
	Text     string `json:"text"`
}

func (k *conn) serve() {
	go k.writer()
	defer k.close()
	sc := bufio.NewScanner(k.pipe)
	sc.Buffer(make([]byte, 64<<10), 16<<20)
	for sc.Scan() {
		var r request
		if json.Unmarshal(sc.Bytes(), &r) != nil {
			return
		}
		if r.Method == "machine.exec" {
			go k.reply(r.ID, k.core.execute(k.ctx, r.Params), nil)
			continue
		}
		res, rerr := k.core.handle(k, r)
		if !k.reply(r.ID, res, rerr) {
			return
		}
	}
}

func (k *conn) reply(id uint64, res any, rerr *rpcError) bool {
	msg := map[string]any{"jsonrpc": "2.0", "id": id}
	if rerr != nil {
		msg["error"] = map[string]any{"code": -32000, "message": rerr.Message, "data": map[string]string{"kind": rerr.Kind}}
	} else {
		msg["result"] = res
	}
	b, err := json.Marshal(msg)
	if err != nil {
		return false
	}
	return k.enqueue(b)
}

func (k *conn) enqueue(b []byte) bool {
	select {
	case k.queue <- b:
		return true
	default:
		go k.close()
		return false
	}
}

func (k *conn) writer() {
	for {
		select {
		case b := <-k.queue:
			if _, err := k.pipe.Write(append(b, '\n')); err != nil {
				k.close()
				return
			}
		case <-k.done:
			return
		}
	}
}

func (k *conn) close() {
	k.once.Do(func() {
		close(k.done)
		k.stop()
		_ = k.pipe.Close()
		k.core.mu.Lock()
		delete(k.core.conns, k)
		k.core.mu.Unlock()
	})
}

func (c *Core) handle(k *conn, r request) (any, *rpcError) {
	c.mu.Lock()
	defer c.mu.Unlock()
	switch r.Method {
	case "hello":
		return map[string]any{"generation": herdr.Generation, "version": "herdrtest"}, nil
	case "terminal.run":
		var q herdr.RunReq
		_ = json.Unmarshal(r.Params, &q)
		return c.addTerm(q)
	case "terminal.list":
		return c.list(r.Params), nil
	case "events.subscribe":
		return c.subscribe(k, r.Params)
	}
	return c.onTerminal(r)
}

func (c *Core) execute(ctx context.Context, params json.RawMessage) any {
	var q herdr.ExecReq
	_ = json.Unmarshal(params, &q)
	c.mu.Lock()
	fn := c.exec
	c.mu.Unlock()
	return fn(ctx, q)
}

func (c *Core) list(params json.RawMessage) any {
	var q struct {
		All    bool              `json:"all"`
		Labels map[string]string `json:"labels"`
	}
	_ = json.Unmarshal(params, &q)
	out := []herdr.Terminal{}
	for _, t := range c.terms {
		if (q.All || t.t.Live()) && matches(t.t.Labels, q.Labels) {
			out = append(out, t.t)
		}
	}
	return map[string]any{"terminals": out, "hidden_finished": 0}
}

// subscribe refuses labels sent as null, as hc 0.1.16 does.
func (c *Core) subscribe(k *conn, params json.RawMessage) (any, *rpcError) {
	var q struct {
		Labels json.RawMessage `json:"labels"`
	}
	_ = json.Unmarshal(params, &q)
	if string(q.Labels) == "null" {
		return nil, &rpcError{Kind: "invalid_params", Message: "invalid type: null, expected a map"}
	}
	_ = json.Unmarshal(q.Labels, &k.sub)
	if k.sub == nil {
		k.sub = map[string]string{}
	}
	out := []herdr.Terminal{}
	for _, t := range c.terms {
		if matches(t.t.Labels, k.sub) {
			out = append(out, t.t)
		}
	}
	return map[string]any{"channel": nil, "seq": c.seq, "terminals": out}, nil
}

func (c *Core) onTerminal(r request) (any, *rpcError) {
	var q target
	_ = json.Unmarshal(r.Params, &q)
	t := c.find(q.Terminal)
	if t == nil {
		return nil, &rpcError{Kind: herdr.KindNotFound, Message: fmt.Sprintf("no terminal %q", q.Terminal)}
	}
	switch r.Method {
	case "terminal.get":
		return t.t, nil
	case "terminal.kill":
		if t.t.Live() {
			sig := 1
			c.finish(t, nil, &sig, "killed")
		}
		return t.t, nil
	case "terminal.send":
		return c.sendTo(t, q)
	case "terminal.interrupt":
		t.intrs++
		return map[string]any{"effect": "none"}, nil
	case "terminal.read":
		return map[string]any{"text": strings.Join(t.sent, "\n"), "complete": true, "how": "screen"}, nil
	}
	return nil, &rpcError{Kind: "unknown_method", Message: r.Method}
}

func (c *Core) sendTo(t *term, q target) (any, *rpcError) {
	switch {
	case !t.t.Live():
		return nil, &rpcError{Kind: "finished", Message: t.t.Address + " has finished"}
	case t.t.Status != nil && t.t.Status.State == "blocked" && !q.Force:
		return nil, &rpcError{Kind: "blocked", Message: t.t.Address + " is blocked"}
	}
	t.sent = append(t.sent, q.Text)
	var status *string
	if t.t.Status != nil {
		status = &t.t.Status.State
	}
	return map[string]any{"started": nil, "status": status, "turn": 0}, nil
}
