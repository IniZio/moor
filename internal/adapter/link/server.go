package link

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"fmt"
	"io"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// KeyStore is what the server needs to know about a runner at hello time. It is read on every
// handshake, so revoking a runner takes effect on its next connect. It returns
// domain.ErrUnknownRunner for an unknown id, and a revoked runner's key with domain.ErrRevokedRunner, so that
// the server can say "revoked" to the holder of that key and to no one else.
type KeyStore interface {
	Lookup(ctx context.Context, id domain.RunnerID) (ed25519.PublicKey, error)
}

// Presence records links durably, so the order of two links is decided by the store and not by a
// process. Linked bumps the runner's generation and marks it online. Unlinked marks it offline only
// if gen is still the stored generation, because during a deploy two server processes hold links to
// the same runner and an in-process counter cannot order them.
type Presence interface {
	Linked(ctx context.Context, id domain.RunnerID, build string) (gen int64, err error)
	Unlinked(ctx context.Context, id domain.RunnerID, gen int64) error
}

// ServerConfig wires the server half. Host is HostOf the public URL runners dial.
type ServerConfig struct {
	Host     string
	Keys     KeyStore
	Presence Presence
	MinProto int
	Timeout  time.Duration
	Rand     io.Reader
}

// Accepted is what a successful server handshake yields.
type Accepted struct {
	Runner domain.RunnerID
	Build  string
	Proto  int
	Gen    int64
}

// Server runs the server half of the handshake on accepted WebSockets.
type Server struct {
	cfg      ServerConfig
	draining atomic.Bool
}

// NewServer fails fast on a config without the parts every handshake needs. Zero MinProto, Timeout
// and Rand take production defaults.
func NewServer(cfg ServerConfig) (*Server, error) {
	if cfg.Host == "" || cfg.Keys == nil || cfg.Presence == nil {
		return nil, errors.New("link: server needs a host, a key store and presence")
	}
	if cfg.MinProto == 0 {
		cfg.MinProto = 1
	}
	if cfg.Timeout == 0 {
		cfg.Timeout = 10 * time.Second
	}
	if cfg.Rand == nil {
		cfg.Rand = rand.Reader
	}
	return &Server{cfg: cfg}, nil
}

// Drain makes every later hello answer relink: a replacement server is up.
func (s *Server) Drain() { s.draining.Store(true) }

// Handshake runs the server half on an accepted WebSocket. Every read and write is bounded by
// Timeout. Only two conditions send a refused frame, an unknown runner and a bad proof, and they
// send the same one. A failure of the server's own (the key store, Presence) closes the socket with
// status 1011 and no frame, so a database blip never makes a runner give up. On error the socket is
// closed here; on success it is left open for the caller.
func (s *Server) Handshake(ctx context.Context, c *websocket.Conn) (Accepted, error) {
	c.SetReadLimit(maxHandshake)
	acc, err := s.run(ctx, hsio{c: c, timeout: s.cfg.Timeout})
	if err != nil {
		code := websocket.StatusInternalError
		if errors.Is(err, ErrRejected) || errors.Is(err, ErrRelinkLater) {
			code = websocket.StatusNormalClosure
		}
		_ = c.Close(code, "")
	}
	return acc, err
}

func (s *Server) run(ctx context.Context, hs hsio) (Accepted, error) {
	hello, err := s.readHello(ctx, hs)
	if err != nil {
		return Accepted{}, err
	}
	if s.draining.Load() {
		_ = hs.write(ctx, wire.KindRelink, wire.Relink{})
		return Accepted{}, ErrRelinkLater
	}
	runner := domain.RunnerID(hello.Runner)
	key, err := s.cfg.Keys.Lookup(ctx, runner)
	revoked := errors.Is(err, domain.ErrRevokedRunner)
	known := err == nil || revoked
	switch {
	case err != nil && !revoked && !errors.Is(err, domain.ErrUnknownRunner):
		return Accepted{}, fmt.Errorf("link: key lookup: %w", err)
	case known && len(key) != ed25519.PublicKeySize:
		return Accepted{}, fmt.Errorf("link: stored key of %s has %d bytes", runner, len(key))
	}
	if err := s.authenticate(ctx, hs, hello, key, known, revoked); err != nil {
		return Accepted{}, err
	}
	gen, err := s.cfg.Presence.Linked(ctx, runner, hello.Build)
	if err != nil {
		return Accepted{}, fmt.Errorf("link: record link: %w", err)
	}
	if err := hs.write(ctx, wire.KindWelcome, wire.Welcome{Proto: hello.Proto, Gen: gen}); err != nil {
		return Accepted{}, errors.Join(err, s.unlinked(ctx, runner, gen))
	}
	return Accepted{Runner: runner, Build: hello.Build, Proto: hello.Proto, Gen: gen}, nil
}

// unlinked undoes a Linked whose welcome never reached the runner. The caller's ctx may be the
// reason the write failed, so it is detached.
func (s *Server) unlinked(ctx context.Context, id domain.RunnerID, gen int64) error {
	ctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), s.cfg.Timeout)
	defer cancel()
	if err := s.cfg.Presence.Unlinked(ctx, id, gen); err != nil {
		return fmt.Errorf("link: offline not recorded: %w", err)
	}
	return nil
}

// readHello reads the runner's first frame. A hello this server cannot read, or whose protocol is
// outside [MinProto, wire.Proto], is answered with update-required: a future runner's hello looks
// like "update me" to an old server, and the reverse.
func (s *Server) readHello(ctx context.Context, hs hsio) (wire.Hello, error) {
	var hello wire.Hello
	err := hs.readAs(ctx, wire.KindHello, &hello)
	switch {
	case err != nil && !errors.Is(err, errBadFrame):
		return wire.Hello{}, err
	case err != nil || hello.Proto < s.cfg.MinProto || hello.Proto > wire.Proto:
		_ = hs.write(ctx, wire.KindUpdateRequired, wire.UpdateRequired{ServerProto: wire.Proto, MinProto: s.cfg.MinProto})
		return wire.Hello{}, ErrRejected
	}
	return hello, nil
}

// authenticate sends a fresh nonce and checks the proof over it, the server's own host and the
// runner id. A revoked runner that proves its key is told so, since it is the one who may know.
func (s *Server) authenticate(ctx context.Context, hs hsio, hello wire.Hello, key ed25519.PublicKey, known, revoked bool) error {
	nonce := make([]byte, 32)
	if _, err := io.ReadFull(s.cfg.Rand, nonce); err != nil {
		return fmt.Errorf("link: nonce: %w", err)
	}
	if err := hs.write(ctx, wire.KindChallenge, wire.Challenge{Nonce: nonce}); err != nil {
		return err
	}
	var pf wire.Proof
	err := hs.readAs(ctx, wire.KindProof, &pf)
	if err != nil && !errors.Is(err, errBadFrame) {
		return err
	}
	claim := proof{Host: s.cfg.Host, Nonce: nonce, Runner: domain.RunnerID(hello.Runner), Proto: hello.Proto}
	reason := ""
	switch {
	case err != nil || !claim.verify(key, known, pf.Sig):
		reason = refusedReason
	case revoked:
		reason = revokedReason
	default:
		return nil
	}
	_ = hs.write(ctx, wire.KindRefused, wire.Refused{Reason: reason})
	return ErrRejected
}
