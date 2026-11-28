package link

import (
	"context"
	"crypto/ed25519"
	"fmt"
	"net/http"
	"net/url"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// AgentConfig wires the runner half. Zero Proto and Timeout take production defaults.
type AgentConfig struct {
	URL     string
	Runner  domain.RunnerID
	Key     ed25519.PrivateKey
	Build   string
	Proto   int
	Timeout time.Duration
	HTTP    *http.Client
}

// Dial opens the WebSocket, runs the runner half of the handshake and returns the socket, still
// open, with the server's welcome. On any failure the socket is closed. The signed host is HostOf
// cfg.URL, never anything the server sent.
func Dial(ctx context.Context, cfg AgentConfig) (*websocket.Conn, wire.Welcome, error) {
	u, err := url.Parse(cfg.URL)
	if err != nil || u.Host == "" {
		return nil, wire.Welcome{}, fmt.Errorf("link: URL %q is not a URL with a host", cfg.URL)
	}
	if cfg.Proto == 0 {
		cfg.Proto = wire.Proto
	}
	if cfg.Timeout == 0 {
		cfg.Timeout = 10 * time.Second
	}
	c, _, err := websocket.Dial(ctx, cfg.URL, &websocket.DialOptions{HTTPClient: cfg.HTTP, CompressionMode: websocket.CompressionDisabled})
	if err != nil {
		return nil, wire.Welcome{}, fmt.Errorf("link: dial: %w", err)
	}
	c.SetReadLimit(maxHandshake)
	w, err := agentHandshake(ctx, hsio{c: c, timeout: cfg.Timeout}, cfg, HostOf(u))
	if err != nil {
		_ = c.Close(websocket.StatusNormalClosure, "")
		return nil, wire.Welcome{}, err
	}
	return c, w, nil
}

func agentHandshake(ctx context.Context, hs hsio, cfg AgentConfig, host string) (wire.Welcome, error) {
	hello := wire.Hello{Runner: string(cfg.Runner), Proto: cfg.Proto, Build: cfg.Build}
	if err := hs.write(ctx, wire.KindHello, hello); err != nil {
		return wire.Welcome{}, err
	}
	var ch wire.Challenge
	if err := hs.readServer(ctx, wire.KindChallenge, &ch); err != nil {
		return wire.Welcome{}, err
	}
	sig := proof{Host: host, Nonce: ch.Nonce, Runner: cfg.Runner, Proto: cfg.Proto}.sign(cfg.Key)
	if err := hs.write(ctx, wire.KindProof, wire.Proof{Sig: sig}); err != nil {
		return wire.Welcome{}, err
	}
	var w wire.Welcome
	if err := hs.readServer(ctx, wire.KindWelcome, &w); err != nil {
		return wire.Welcome{}, err
	}
	return w, nil
}

// readServer reads the server's next frame. One of the wanted kind is decoded into v. Any other is
// turned into what it means: update-required, a refusal or a relink ask. A kind this build does not
// know means a newer server spoke, which reads like update-required.
func (h hsio) readServer(ctx context.Context, want wire.Kind, v any) error {
	f, err := h.read(ctx)
	if err != nil {
		return err
	}
	if f.Kind == want {
		return f.decode(v)
	}
	return f.unexpected()
}

func (f frame) unexpected() error {
	if f.Kind == wire.KindUpdateRequired {
		var u wire.UpdateRequired
		if err := f.decode(&u); err != nil {
			return err
		}
		return &UpdateRequiredError{ServerProto: u.ServerProto, MinProto: u.MinProto}
	}
	if f.Kind == wire.KindRefused {
		var r wire.Refused
		if err := f.decode(&r); err != nil {
			return err
		}
		return &RefusedError{Reason: r.Reason}
	}
	if f.Kind == wire.KindRelink {
		return ErrRelinkLater
	}
	if f.Kind == wire.KindChallenge || f.Kind == wire.KindWelcome {
		return errProtocol
	}
	return &UpdateRequiredError{}
}
