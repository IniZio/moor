package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/herdr/herdrtest"
	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/adapter/link/linktest"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

var partNames = []string{"boxproxy", "relay", "box", "deskproxy", "desk"}

// fakeHost stands in for the Docker host: it outlives every runner a test starts, as the host does.
type fakeHost struct {
	mu    sync.Mutex
	tasks map[string]runner.Held
	ups   int
}

func (h *fakeHost) Up(_ context.Context, b runner.Box) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	cur, ok := h.tasks[b.Fence.Task]
	if ok && wire.Newer(cur.Fence, b.Fence) {
		return errors.New("fake host: stale key")
	}
	if ok && cur.Fence == b.Fence {
		return nil
	}
	h.ups++
	held := runner.Held{Fence: b.Fence}
	for _, p := range partNames {
		held.Parts = append(held.Parts, runner.Part{Name: p, Container: "c-" + b.Fence.Task + "-" + p, State: "running"})
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

// desks is one fake herdr-core per task, as each task's desk runs its own.
type desks struct {
	mu    sync.Mutex
	cores map[string]*herdrtest.Core
}

func (d *desks) core(task string) *herdrtest.Core {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.cores[task]
	if c == nil {
		c = herdrtest.New()
		d.cores[task] = c
	}
	return c
}

func (d *desks) connect(_ context.Context, task string) (io.ReadWriteCloser, error) {
	return d.core(task).Connect(), nil
}

type rig struct {
	t      *testing.T
	ctx    context.Context
	portal *linktest.Portal
	host   *fakeHost
	desks  *desks
}

func newRig(t *testing.T) *rig { return rigWithin(t, 20*time.Second) }

func rigWithin(t *testing.T, limit time.Duration) *rig {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), limit)
	t.Cleanup(cancel)
	return &rig{t: t, ctx: ctx, portal: linktest.New(t), host: &fakeHost{tasks: map[string]runner.Held{}}, desks: &desks{cores: map[string]*herdrtest.Core{}}}
}

// start runs a runner and returns the portal's end of its link and a stop that ends the runner
// as a process exit would, and checks it returned cleanly.
func (g *rig) start(h runner.Host, connect func(context.Context, string) (io.ReadWriteCloser, error)) (*link.Session, func()) {
	g.t.Helper()
	r := newRunner(g.portal.Agent(), h, sessions{connect: connect, argv: []string{"harness"}}, []string{"api.example.com"})
	ctx, cancel := context.WithCancel(g.ctx)
	done := make(chan error, 1)
	go func() { done <- r.Run(ctx) }()
	s, err := g.portal.Next(g.ctx)
	if err != nil {
		g.t.Fatal(err)
	}
	return s, func() {
		cancel()
		if err := <-done; err != nil {
			g.t.Errorf("runner ended with %v", err)
		}
	}
}

func (g *rig) startFake() (*link.Session, func()) { return g.start(g.host, g.desks.connect) }

func call(ctx context.Context, s *link.Session, op wire.Kind, body, out any) error {
	st, err := s.Open(ctx, op, body, out)
	if err != nil {
		return fmt.Errorf("%s: %w", op, err)
	}
	if err := st.Close(); err != nil {
		return fmt.Errorf("%s: %w", op, err)
	}
	return nil
}

func startMsg(task string, run, gen int64) wire.TaskStart {
	return wire.TaskStart{Fence: wire.Fence{Task: task, Run: run, Gen: gen}, Prompt: "fix the build", AllowHosts: []string{"registry.npmjs.org"}, Harness: "stand-in"}
}

// watch opens task.watch and delivers its lines until the test ends.
func watch(t *testing.T, ctx context.Context, s *link.Session) <-chan wire.TaskWatchLine {
	t.Helper()
	st, err := s.Open(ctx, wire.KindTaskWatch, wire.TaskWatch{}, nil)
	if err != nil {
		t.Fatal(err)
	}
	lines := make(chan wire.TaskWatchLine, 64)
	go func() {
		defer close(lines)
		for {
			var l wire.TaskWatchLine
			if st.Recv(&l) != nil {
				return
			}
			lines <- l
		}
	}()
	return lines
}

// await reads lines until one matches, failing the test if the stream ends or ctx does first.
func await(t *testing.T, ctx context.Context, lines <-chan wire.TaskWatchLine, what string, match func(wire.TaskWatchLine) bool) wire.TaskWatchLine {
	t.Helper()
	for {
		select {
		case l, ok := <-lines:
			if !ok {
				t.Fatalf("the watch ended before %s", what)
			}
			if match(l) {
				return l
			}
		case <-ctx.Done():
			t.Fatalf("no line for %s", what)
		}
	}
}

func sessionIs(task, state string) func(wire.TaskWatchLine) bool {
	return func(l wire.TaskWatchLine) bool {
		return l.Task == task && l.Part == runner.SessionPart && l.State == state
	}
}

func partIs(task, part, state string) func(wire.TaskWatchLine) bool {
	return func(l wire.TaskWatchLine) bool { return l.Task == task && l.Part == part && l.State == state }
}
