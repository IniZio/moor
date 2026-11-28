// Package linktest is a fake portal end of the runner link: the real handshake server on an
// httptest server, with the runner's key and its presence in memory. Each link a runner makes is
// handed to the test as a session, so the test plays the portal's side of every op.
package linktest

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/domain"
)

// Runner is the id the fake portal knows.
const Runner domain.RunnerID = "runner-1"

type Portal struct {
	URL   string
	key   ed25519.PrivateKey
	links chan *link.Session
	// revoked makes the key store answer as it does for a revoked runner.
	revoked atomic.Bool

	mu   sync.Mutex
	open []*link.Session
	gen  int64
}

// New starts the fake portal. It closes every link and the server when the test ends.
func New(t testing.TB) *Portal {
	t.Helper()
	pub, key, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	p := &Portal{key: key, links: make(chan *link.Session, 8)}
	srv := httptest.NewServer(nil)
	u, _ := url.Parse(srv.URL)
	hs, err := link.NewServer(link.ServerConfig{Host: link.HostOf(u), Keys: keys{p: p, pub: pub}, Presence: p})
	if err != nil {
		t.Fatal(err)
	}
	srv.Config.Handler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { p.serve(w, r, hs) })
	p.URL = "ws" + srv.URL[len("http"):]
	t.Cleanup(func() {
		p.Drop()
		srv.Close()
	})
	return p
}

// Revoke makes the portal refuse the runner's key from now on with the reason revoked, as the real
// portal does once a person revokes the machine. Links already open stay open until Drop.
func (p *Portal) Revoke() { p.revoked.Store(true) }

// Agent is the runner's config for this portal.
func (p *Portal) Agent() link.AgentConfig {
	return link.AgentConfig{URL: p.URL, Runner: Runner, Key: p.key, Build: "test"}
}

// Next waits for the runner's next link.
func (p *Portal) Next(ctx context.Context) (*link.Session, error) {
	select {
	case s := <-p.links:
		return s, nil
	case <-ctx.Done():
		return nil, fmt.Errorf("linktest: no link: %w", ctx.Err())
	}
}

// Drop closes every open link, as a portal restart does.
func (p *Portal) Drop() {
	p.mu.Lock()
	open := p.open
	p.open = nil
	p.mu.Unlock()
	for _, s := range open {
		_ = s.Close()
	}
}

func (p *Portal) serve(w http.ResponseWriter, r *http.Request, hs *link.Server) {
	c, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	if _, err := hs.Handshake(r.Context(), c); err != nil {
		return
	}
	s, err := link.NewSession(r.Context(), c, false)
	if err != nil {
		return
	}
	p.mu.Lock()
	p.open = append(p.open, s)
	p.mu.Unlock()
	p.links <- s
	<-s.Done()
}

func (p *Portal) Linked(context.Context, domain.RunnerID, string) (int64, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.gen++
	return p.gen, nil
}

func (p *Portal) Unlinked(context.Context, domain.RunnerID, int64) error { return nil }

type keys struct {
	p   *Portal
	pub ed25519.PublicKey
}

func (k keys) Lookup(_ context.Context, id domain.RunnerID) (ed25519.PublicKey, error) {
	switch {
	case id != Runner:
		return nil, domain.ErrUnknownRunner
	case k.p.revoked.Load():
		return k.pub, domain.ErrRevokedRunner
	}
	return k.pub, nil
}
