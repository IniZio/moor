package link

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/wire"
)

// maxHandshake bounds one handshake frame. It is set before the socket carries anything else.
const maxHandshake = 64 << 10

// ErrRejected means the server answered the peer with a refusal or update-required and closed.
var ErrRejected = errors.New("link: handshake rejected")

// ErrRelinkLater is the runner's view of a draining server's answer to its hello.
var ErrRelinkLater = errors.New("link: server is draining, retry shortly")

var (
	errBadFrame = errors.New("link: unreadable handshake frame")
	errProtocol = errors.New("link: unexpected handshake frame")
)

// RefusedError is why a handshake ended in a refusal.
type RefusedError struct{ Reason string }

func (e *RefusedError) Error() string { return "link: server refused this runner: " + e.Reason }

// UpdateRequiredError carries both versions so the operator sees which side is stale.
type UpdateRequiredError struct{ ServerProto, MinProto int }

func (e *UpdateRequiredError) Error() string {
	return fmt.Sprintf("link: protocol mismatch (server speaks %d, accepts from %d, this build speaks %d)", e.ServerProto, e.MinProto, wire.Proto)
}

// frame is the JSON of every handshake frame: a kind and a body. Decoding is lenient, so an older
// peer ignores fields a newer one adds.
type frame struct {
	Kind wire.Kind       `json:"kind"`
	Body json.RawMessage `json:"body,omitempty"`
}

func (f frame) decode(v any) error {
	if len(f.Body) == 0 {
		return nil
	}
	if json.Unmarshal(f.Body, v) != nil {
		return errBadFrame
	}
	return nil
}

// hsio reads and writes handshake frames as WebSocket text frames, each bounded by one timeout.
type hsio struct {
	c       *websocket.Conn
	timeout time.Duration
}

func (h hsio) read(ctx context.Context) (frame, error) {
	ctx, cancel := context.WithTimeout(ctx, h.timeout)
	defer cancel()
	typ, data, err := h.c.Read(ctx)
	if err != nil {
		return frame{}, fmt.Errorf("link: read handshake: %w", err)
	}
	var f frame
	if typ != websocket.MessageText || json.Unmarshal(data, &f) != nil {
		return frame{}, errBadFrame
	}
	return f, nil
}

func (h hsio) write(ctx context.Context, kind wire.Kind, body any) error {
	raw, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("link: encode handshake: %w", err)
	}
	data, err := json.Marshal(frame{Kind: kind, Body: raw})
	if err != nil {
		return fmt.Errorf("link: encode handshake: %w", err)
	}
	ctx, cancel := context.WithTimeout(ctx, h.timeout)
	defer cancel()
	if err := h.c.Write(ctx, websocket.MessageText, data); err != nil {
		return fmt.Errorf("link: write handshake: %w", err)
	}
	return nil
}

// readAs reads one frame that must be of kind and decodes its body into v. A frame that is
// unreadable or of another kind is errBadFrame.
func (h hsio) readAs(ctx context.Context, kind wire.Kind, v any) error {
	f, err := h.read(ctx)
	if err != nil {
		return err
	}
	if f.Kind != kind {
		return errBadFrame
	}
	return f.decode(v)
}
