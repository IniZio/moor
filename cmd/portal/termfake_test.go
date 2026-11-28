package main

import (
	"context"
	"io"
	"strings"
	"sync"

	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

const peerSecret = "replicas-share-this"

func (s *fakeSessions) Attach(_ context.Context, a wire.TermAttach) (runner.TTY, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := &echoTTY{mode: a.Mode, out: make(chan []byte, 16), closed: make(chan struct{})}
	s.viewers = append(s.viewers, t)
	return t, nil
}

func (s *fakeSessions) Screen(context.Context, string, int) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return strings.Join(s.typed, "\n"), nil
}

func (s *fakeSessions) Type(_ context.Context, _, text string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.typed = append(s.typed, text)
	return nil
}

// keysOf is every key that reached a viewer's terminal in mode.
func (s *fakeSessions) keysOf(mode string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	var all strings.Builder
	for _, v := range s.viewers {
		if v.mode == mode {
			all.WriteString(v.keys())
		}
	}
	return all.String()
}

type echoTTY struct {
	mode   string
	out    chan []byte
	closed chan struct{}
	once   sync.Once
	mu     sync.Mutex
	got    strings.Builder
}

func (t *echoTTY) Read(p []byte) (int, error) {
	select {
	case b := <-t.out:
		return copy(p, b), nil
	case <-t.closed:
		return 0, io.EOF
	}
}

func (t *echoTTY) Write(p []byte) (int, error) {
	t.mu.Lock()
	t.got.Write(p)
	t.mu.Unlock()
	t.out <- append([]byte("echo:"), p...)
	return len(p), nil
}

func (t *echoTTY) keys() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.got.String()
}

func (t *echoTTY) Resize(wire.Size) error { return nil }

func (t *echoTTY) Close() error {
	t.once.Do(func() { close(t.closed) })
	return nil
}
