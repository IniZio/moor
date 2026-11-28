package main

import (
	"context"
	"errors"
	"io"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

// ttys hands out echoing terminals and keeps each with the command it was asked to run.
type ttys struct {
	mu  sync.Mutex
	all []*fakeTTY
}

func (s *ttys) attach(_ context.Context, _ string, size wire.Size, argv ...string) (runner.TTY, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := &fakeTTY{argv: argv, out: make(chan []byte, 16), closed: make(chan struct{}), sizes: []wire.Size{size}}
	s.all = append(s.all, t)
	return t, nil
}

func (s *ttys) get(i int) *fakeTTY {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.all[i]
}

// fakeTTY echoes what is typed into it, as a terminal in cooked mode does.
type fakeTTY struct {
	argv   []string
	out    chan []byte
	closed chan struct{}
	once   sync.Once
	mu     sync.Mutex
	sizes  []wire.Size
}

func (t *fakeTTY) Read(p []byte) (int, error) {
	select {
	case b := <-t.out:
		return copy(p, b), nil
	case <-t.closed:
		return 0, io.EOF
	}
}

func (t *fakeTTY) Write(p []byte) (int, error) {
	t.out <- append([]byte("echo:"), p...)
	return len(p), nil
}

func (t *fakeTTY) Resize(s wire.Size) error {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.sizes = append(t.sizes, s)
	return nil
}

func (t *fakeTTY) Close() error {
	t.once.Do(func() { close(t.closed) })
	return nil
}

func (t *fakeTTY) lastSize() wire.Size {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.sizes[len(t.sizes)-1]
}

func attach(t *testing.T, g *rig, s *link.Session, mode string) *link.Stream {
	t.Helper()
	st, err := s.Open(g.ctx, wire.KindTermAttach, wire.TermAttach{Task: "t1", Mode: mode, Cols: 80, Rows: 24, History: 1000}, nil)
	if err != nil {
		t.Fatal(err)
	}
	return st
}

func send(t *testing.T, st *link.Stream, kind wire.TermFrame, b []byte) {
	t.Helper()
	if err := wire.WriteTerm(st, kind, b); err != nil {
		t.Fatal(err)
	}
}

func expect(t *testing.T, st *link.Stream, kind wire.TermFrame, body string) {
	t.Helper()
	k, b, err := wire.ReadTerm(st)
	if err != nil || k != kind || string(b) != body {
		t.Fatalf("frame = %d %q %v, want %d %q", k, b, err, kind, body)
	}
}

func TestAWriterTypesAndSizesTheSessionAndAnObserverOnlyWatches(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	writer := attach(t, g, s, wire.TermWrite)
	watcher := attach(t, g, s, wire.TermObserve)
	if w, o := g.ttys.get(0).argv, g.ttys.get(1).argv; !slices.Equal(w, []string{"hc", "attach", sessionName, "--history", "1000"}) || !slices.Contains(o, "--observe") {
		t.Fatalf("the viewers ran %q and %q", w, o)
	}
	send(t, writer, wire.TermIn, []byte("ls\r"))
	expect(t, writer, wire.TermOut, "echo:ls\r")
	send(t, writer, wire.TermResize, wire.Size{Cols: 120, Rows: 40}.Bytes())
	send(t, watcher, wire.TermIn, []byte("rm -rf /\r"))
	send(t, watcher, wire.TermResize, wire.Size{Cols: 10, Rows: 5}.Bytes())
	send(t, writer, wire.TermIn, []byte("x"))
	expect(t, writer, wire.TermOut, "echo:x")
	if got := g.ttys.get(0).lastSize(); got != (wire.Size{Cols: 120, Rows: 40}) {
		t.Errorf("the writer's size is %v", got)
	}
	if got := g.ttys.get(1).lastSize(); got != (wire.Size{Cols: 80, Rows: 24}) {
		t.Errorf("the watcher's resize reached its terminal: %v", got)
	}
	_ = watcher.Close()
	<-g.ttys.get(1).closed
	_ = g.ttys.get(0).Close()
	expect(t, writer, wire.TermExit, "")
	if terms := g.desks.core("t1").Terminals(); len(terms) != 1 || !terms[0].Live() {
		t.Fatalf("viewers going ended the session: %+v", terms)
	}
}

func TestTheScreenAndTypedLinesGoThroughTheDeskAndAnUnknownTaskIsRefused(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	if err := call(g.ctx, s, wire.KindTermSend, wire.TermSend{Task: "t1", Text: "please add a test"}, nil); err != nil {
		t.Fatal(err)
	}
	var screen wire.TermReadReply
	if err := call(g.ctx, s, wire.KindTermRead, wire.TermRead{Task: "t1", Lines: 50}, &screen); err != nil || !strings.Contains(screen.Text, "please add a test") {
		t.Fatalf("screen %q, %v", screen.Text, err)
	}
	for _, op := range []struct {
		kind wire.Kind
		body any
	}{
		{wire.KindTermRead, wire.TermRead{Task: "nope"}},
		{wire.KindTermSend, wire.TermSend{Task: "nope", Text: "hi"}},
		{wire.KindTermAttach, wire.TermAttach{Task: "nope", Mode: wire.TermWrite, Cols: 80, Rows: 24}},
	} {
		if err := call(g.ctx, s, op.kind, op.body, nil); !errors.Is(err, &wire.Error{Code: wire.CodeRefused}) {
			t.Errorf("%s of an unknown task: %v", op.kind, err)
		}
	}
	if err := call(g.ctx, s, wire.KindTermAttach, wire.TermAttach{Task: "t1", Mode: "steer", Cols: 80, Rows: 24}, nil); !errors.Is(err, &wire.Error{Code: wire.CodeBadRequest}) {
		t.Errorf("an attach in an unknown mode: %v", err)
	}
}
