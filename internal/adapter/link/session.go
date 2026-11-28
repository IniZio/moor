package link

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"time"

	"github.com/coder/websocket"
	"github.com/hashicorp/yamux"

	"github.com/IniZio/moor/internal/wire"
)

const (
	// keepAlive and writeTimeout bound how long a dead link goes unnoticed: a ping every keepAlive
	// that gets no answer within writeTimeout ends the session.
	keepAlive    = 5 * time.Second
	writeTimeout = 10 * time.Second
	// maxMessage bounds one WebSocket message once the handshake is over. yamux never writes more
	// than its stream window plus a header in one message.
	maxMessage = 2 << 20
)

// Session carries the link's streams over one handshaken WebSocket, multiplexed by yamux. Either
// side opens a stream. Frame 1 is the opener's Request, frame 2 the other side's Reply. After that
// the op decides what flows.
type Session struct{ mux *yamux.Session }

// NewSession starts the multiplexer on a WebSocket whose handshake is done. The runner end is the
// client, the portal end is not. ctx bounds the session's life: when it ends, the session closes.
func NewSession(ctx context.Context, c *websocket.Conn, client bool) (*Session, error) {
	c.SetReadLimit(maxMessage)
	nc := websocket.NetConn(ctx, c, websocket.MessageBinary)
	cfg := yamux.DefaultConfig()
	cfg.KeepAliveInterval = keepAlive
	cfg.ConnectionWriteTimeout = writeTimeout
	cfg.LogOutput = io.Discard
	start := yamux.Server
	if client {
		start = yamux.Client
	}
	mux, err := start(nc, cfg)
	if err != nil {
		_ = nc.Close()
		return nil, fmt.Errorf("link: session: %w", err)
	}
	return &Session{mux: mux}, nil
}

// Done is closed once the session has ended, by either side or by a dead link.
func (s *Session) Done() <-chan struct{} { return s.mux.CloseChan() }

func (s *Session) Close() error {
	if err := s.mux.Close(); err != nil {
		return fmt.Errorf("link: close session: %w", err)
	}
	return nil
}

// Accept waits for the peer's next stream and reads its request. A stream whose request cannot be
// read is answered with bad_request and closed, and Accept waits for the next one.
func (s *Session) Accept(ctx context.Context) (*Stream, error) {
	for {
		y, err := s.mux.AcceptStreamWithContext(ctx)
		if err != nil {
			return nil, fmt.Errorf("link: accept: %w", err)
		}
		st := &Stream{conn: y, stop: func() bool { return true }}
		if err := ReadFrame(y, &st.req, true); err == nil {
			return st, nil
		}
		_ = st.Fail(&wire.Error{Code: wire.CodeBadRequest, Msg: "unreadable request"})
		_ = st.Close()
	}
}

// Open opens a stream for op, sends body and reads the reply into out. A reply with an error
// closes the stream and returns that error as a *wire.Error. ctx bounds the whole stream, not only
// the call: when it ends the stream closes.
func (s *Session) Open(ctx context.Context, op wire.Kind, body, out any) (*Stream, error) {
	y, err := s.mux.OpenStream()
	if err != nil {
		return nil, fmt.Errorf("link: open %s: %w", op, err)
	}
	st := &Stream{conn: y, req: Request{Op: op}}
	st.stop = context.AfterFunc(ctx, func() { _ = y.Close() })
	if err := st.call(body, out); err != nil {
		_ = st.Close()
		return nil, errors.Join(err, ctx.Err())
	}
	return st, nil
}

func (st *Stream) call(body, out any) error {
	raw, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("link: encode %s: %w", st.req.Op, err)
	}
	st.req.Body = raw
	if err := WriteFrame(st.conn, st.req); err != nil {
		return err
	}
	var rep Reply
	if err := ReadFrame(st.conn, &rep, false); err != nil {
		return err
	}
	if rep.Err != nil {
		return rep.Err
	}
	if out == nil || len(rep.Body) == 0 {
		return nil
	}
	if err := json.Unmarshal(rep.Body, out); err != nil {
		return fmt.Errorf("link: decode %s reply: %w", st.req.Op, err)
	}
	return nil
}

// Stream is one op on a session.
type Stream struct {
	conn *yamux.Stream
	req  Request
	stop func() bool
}

// Op is the op the opener asked for.
func (st *Stream) Op() wire.Kind { return st.req.Op }

// Decode reads the request body into v. Unknown fields are refused: a field this side ignores
// could silently weaken the op.
func (st *Stream) Decode(v any) error {
	body := st.req.Body
	if len(body) == 0 {
		body = []byte("{}")
	}
	dec := json.NewDecoder(bytes.NewReader(body))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		return fmt.Errorf("link: decode %s: %w", st.req.Op, err)
	}
	return nil
}

// Reply answers the request with body.
func (st *Stream) Reply(body any) error {
	raw, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("link: encode %s reply: %w", st.req.Op, err)
	}
	return WriteFrame(st.conn, Reply{Body: raw})
}

// Fail answers the request with an error. The caller closes the stream after it.
func (st *Stream) Fail(e *wire.Error) error { return WriteFrame(st.conn, Reply{Err: e}) }

// Send writes one more frame after the reply, such as a task.watch line.
func (st *Stream) Send(v any) error { return WriteFrame(st.conn, v) }

// Recv reads one frame Send wrote. It is lenient, like a reply.
func (st *Stream) Recv(v any) error { return ReadFrame(st.conn, v, false) }

func (st *Stream) Close() error {
	st.stop()
	if err := st.conn.Close(); err != nil {
		return fmt.Errorf("link: close stream: %w", err)
	}
	return nil
}
