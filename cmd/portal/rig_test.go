package main

import (
	"context"
	"crypto/ed25519"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/wire"
)

// replica is one portal process on a real listener, so a runner can dial it and a restart can take
// the same address again.
type replica struct {
	*app
	addr string
	srv  *http.Server
	stop func()
}

func startReplica(t *testing.T, dsn, addr string, opts ...func(*config)) *replica {
	t.Helper()
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	addr = ln.Addr().String()
	a := newAppOn(t, dsn, append([]func(*config){func(c *config) {
		c.publicURL, c.linkTick = "http://"+addr, 50*time.Millisecond
		c.internalURL, c.internalSecret = "http://"+addr, peerSecret
	}}, opts...)...)
	srv := &http.Server{Handler: a.handler, ReadHeaderTimeout: 5 * time.Second}
	go func() { _ = srv.Serve(ln) }()
	r := &replica{app: a, addr: addr, srv: srv}
	var once sync.Once
	r.stop = func() {
		once.Do(func() {
			_ = srv.Close()
			a.close()
		})
	}
	t.Cleanup(r.stop)
	return r
}

// fakeHost stands in for a Docker host. Ups counts the launches of each task it was asked to make.
type fakeHost struct {
	mu    sync.Mutex
	tasks map[string]runner.Held
	ups   map[string]int
}

func newFakeHost() *fakeHost {
	return &fakeHost{tasks: map[string]runner.Held{}, ups: map[string]int{}}
}

func (h *fakeHost) Up(_ context.Context, b runner.Box) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if len(b.BoxAllow) == 0 {
		return errors.New("fake host: no allowed hosts")
	}
	if cur, ok := h.tasks[b.Fence.Task]; ok && cur.Fence == b.Fence {
		return nil
	}
	h.ups[b.Fence.Task]++
	held := runner.Held{Fence: b.Fence}
	for _, p := range []string{"boxproxy", "relay", "box", "deskproxy", "desk"} {
		held.Parts = append(held.Parts, runner.Part{Name: p, Container: b.Fence.Task + "-" + p, State: "running"})
	}
	h.tasks[b.Fence.Task] = held
	return nil
}

func (h *fakeHost) Down(_ context.Context, f wire.Fence) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	delete(h.tasks, f.Task)
	return nil
}

func (h *fakeHost) List(context.Context) ([]runner.Held, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]runner.Held, 0, len(h.tasks))
	for _, t := range h.tasks {
		out = append(out, t)
	}
	slices.SortFunc(out, func(a, b runner.Held) int { return strings.Compare(a.Fence.Task, b.Fence.Task) })
	return out, nil
}

func (h *fakeHost) holds(task string) bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	_, ok := h.tasks[task]
	return ok
}

func (h *fakeHost) launches(task string) int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.ups[task]
}

// fakeSessions reports every session live until the test ends it. Each viewer gets a terminal that
// echoes its keys, and typed lines make up the screen.
type fakeSessions struct {
	mu      sync.Mutex
	ends    map[string]chan struct{}
	typed   []string
	viewers []*echoTTY
}

func (s *fakeSessions) end(task string) chan struct{} {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ends == nil {
		s.ends = map[string]chan struct{}{}
	}
	c := s.ends[task]
	if c == nil {
		c = make(chan struct{})
		s.ends[task] = c
	}
	return c
}

func (s *fakeSessions) Start(context.Context, wire.TaskStart) error { return nil }

func (s *fakeSessions) Watch(ctx context.Context, task string, on func(runner.SessionState)) error {
	on(runner.SessionState{State: "live"})
	select {
	case <-s.end(task):
		on(runner.SessionState{State: wire.SessionFinished, Code: 1})
	case <-ctx.Done():
	}
	<-ctx.Done()
	return fmt.Errorf("watch: %w", ctx.Err())
}

type runnerLink struct{ *link.Session }

func (l runnerLink) Accept(ctx context.Context) (runner.Stream, error) {
	st, err := l.Session.Accept(ctx)
	if err != nil {
		return nil, fmt.Errorf("accept: %w", err)
	}
	return st, nil
}

func dialer(cfg link.AgentConfig) func(ctx context.Context) (runner.Link, error) {
	return func(ctx context.Context) (runner.Link, error) {
		c, _, err := link.Dial(ctx, cfg)
		var refused *link.RefusedError
		switch {
		case errors.As(err, &refused):
			return nil, fmt.Errorf("%w: %w", runner.ErrFatal, err)
		case err != nil:
			return nil, fmt.Errorf("dial: %w", err)
		}
		s, err := link.NewSession(ctx, c, true)
		if err != nil {
			return nil, fmt.Errorf("session: %w", err)
		}
		return runnerLink{s}, nil
	}
}

// machineRig is one enrolled machine: its host, its sessions and the runner that serves them.
type machineRig struct {
	id       string
	key      ed25519.PrivateKey
	host     *fakeHost
	sessions *fakeSessions
	done     chan error
	stop     context.CancelFunc
}

// enrol adds a machine as Mia on r and joins it with a fresh key under name.
func enrol(t *testing.T, r *replica, name string) *machineRig {
	t.Helper()
	id, token := r.addMachine("Mia")
	body, key := joinBody(t, id, token, name)
	r.do(joinReq(body)).has(t, http.StatusOK)
	return &machineRig{id: id, key: key, host: newFakeHost(), sessions: &fakeSessions{}}
}

// run starts the machine's runner against the replica at addr.
func (m *machineRig) run(t *testing.T, addr string) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	agent := link.AgentConfig{URL: "ws://" + addr + "/runner/link", Runner: domain.RunnerID(m.id), Key: m.key, Build: "test"}
	r := runner.New(runner.Config{Dial: dialer(agent), Host: m.host, Sessions: m.sessions, DeskAllow: []string{"api.example.com"}, Slots: 1})
	m.done, m.stop = make(chan error, 1), cancel
	go func() { m.done <- r.Run(ctx) }()
	t.Cleanup(func() {
		cancel()
		<-m.done
	})
}

func (a *app) tasks() (*task.Service, *store.Store) {
	a.t.Helper()
	db, err := store.Open(context.Background(), a.dsn)
	if err != nil {
		a.t.Fatal(err)
	}
	a.t.Cleanup(db.Close)
	return task.New(db, systemClock{}, task.Config{}), db
}

// startTask starts a task as Mia through the portal and returns its id.
func (a *app) startTask(prompt string) domain.TaskID {
	a.t.Helper()
	r := a.do(startJSON("Mia", "example-org/shop-web", prompt))
	r.has(a.t, http.StatusCreated)
	var out struct {
		Task struct{ ID string } `json:"task"`
	}
	if err := json.Unmarshal([]byte(r.body), &out); err != nil {
		a.t.Fatal(err)
	}
	return domain.TaskID(out.Task.ID)
}

func eventually(t *testing.T, what string, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for !ok() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func stateIs(t *testing.T, db *store.Store, id domain.TaskID, want domain.State) func() bool {
	return func() bool {
		got, err := db.Load(context.Background(), id)
		if err != nil {
			t.Fatal(err)
		}
		return got.State == want
	}
}
