package web

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// Terminals reaches a task's session over its machine's link. TermHolder says whether this replica
// holds that link, and if not, the holder another replica named for itself.
type Terminals interface {
	TermHolder(ctx context.Context, id domain.TaskID) (holder string, here bool, err error)
	TermAttach(ctx context.Context, id domain.TaskID, a wire.TermAttach) (io.ReadWriteCloser, error)
	Screen(ctx context.Context, id domain.TaskID, lines int) (string, error)
	Type(ctx context.Context, id domain.TaskID, text string) error
}

const (
	historyLines = 1000
	pingEvery    = 30 * time.Second
	// closeSeatTaken closes a connection whose steering seat another connection took. The browser
	// attaches again to watch.
	closeSeatTaken websocket.StatusCode = 4002
	maxMessage                          = 1 << 20
	maxSide                             = 1000
)

type termEnded struct {
	Ended string `json:"ended"`
}

type termSize struct {
	Cols int `json:"cols"`
	Rows int `json:"rows"`
}

func taskID(r *http.Request) domain.TaskID { return domain.TaskID(chi.URLParam(r, "id")) }

// terminal serves GET /api/tasks/{id}/terminal. The websocket checks the Origin against the host or a
// trusted origin. When another replica holds the link, the connection hops there.
func (s *server) terminal(w http.ResponseWriter, r *http.Request) {
	id := taskID(r)
	holder, here, err := s.deps.Terms.TermHolder(r.Context(), id)
	if err == nil && !here {
		err = s.hop(w, r, holder, "/internal/terminal/"+string(id))
	}
	if err != nil || here {
		s.relay(w, r, id, err, &websocket.AcceptOptions{OriginPatterns: s.originHosts})
	}
}

// relay serves one terminal connection on the replica that holds the link: it takes a seat, attaches
// to the session and carries bytes both ways until either end goes.
func (s *server) relay(w http.ResponseWriter, r *http.Request, id domain.TaskID, routeErr error, opts *websocket.AcceptOptions) {
	rc := http.NewResponseController(w)
	_, _ = rc.SetReadDeadline(time.Time{}), rc.SetWriteDeadline(time.Time{})
	c, err := websocket.Accept(w, r, opts)
	if err != nil {
		return
	}
	defer func() { _ = c.CloseNow() }()
	c.SetReadLimit(maxMessage)
	if routeErr != nil {
		end(r.Context(), c, routeErr)
		return
	}
	steer := r.URL.Query().Get("mode") == seatSteer
	v := s.seats.join(id, personOf(r.Context()), steer)
	defer s.seats.leave(id, v)
	st, err := s.deps.Terms.TermAttach(r.Context(), id, attachOf(r.URL.Query(), steer))
	if err != nil {
		end(r.Context(), c, err)
		return
	}
	defer func() { _ = st.Close() }()
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	go func() { defer cancel(); termOut(ctx, c, st) }()
	go func() { defer cancel(); tellSeat(ctx, c, v) }()
	go ping(ctx, c)
	s.termIn(ctx, c, st, id, v)
}

func attachOf(q url.Values, steer bool) wire.TermAttach {
	a := wire.TermAttach{Mode: wire.TermObserve, Cols: side(q.Get("cols"), 80), Rows: side(q.Get("rows"), 24), History: historyLines}
	if steer {
		a.Mode = wire.TermWrite
	}
	return a
}

func side(v string, fallback uint16) uint16 {
	n, err := strconv.Atoi(v)
	if err != nil || n < 1 || n > maxSide {
		return fallback
	}
	return uint16(n)
}

// end tells the browser why there is no terminal and closes the connection.
func end(ctx context.Context, c *websocket.Conn, err error) {
	reason := "the terminal could not reach the session"
	if errors.Is(err, domain.ErrNoSession) {
		reason = "the task has no live session"
	}
	_ = writeText(ctx, c, termEnded{Ended: reason})
	_ = c.Close(websocket.StatusNormalClosure, "")
}

func writeText(ctx context.Context, c *websocket.Conn, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err //nolint:wrapcheck // only a notice the caller drops
	}
	return c.Write(ctx, websocket.MessageText, b) //nolint:wrapcheck // the caller drops a dead connection
}

// termOut sends the session's output to the browser until the stream ends.
func termOut(ctx context.Context, c *websocket.Conn, st io.Reader) {
	for {
		kind, b, err := wire.ReadTerm(st)
		switch {
		case err != nil:
			return
		case kind == wire.TermExit:
			end(ctx, c, domain.ErrNoSession)
			return
		case kind == wire.TermOut:
			if c.Write(ctx, websocket.MessageBinary, b) != nil {
				return
			}
		}
	}
}

// tellSeat sends each seat notice, and closes the connection once another one takes its seat.
func tellSeat(ctx context.Context, c *websocket.Conn, v *viewer) {
	for {
		select {
		case n := <-v.notices:
			if writeText(ctx, c, n) != nil {
				return
			}
		case <-v.dropped:
			_ = c.Close(closeSeatTaken, "another connection took the keyboard")
			return
		case <-ctx.Done():
			return
		}
	}
}

func ping(ctx context.Context, c *websocket.Conn) {
	t := time.NewTicker(pingEvery)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if c.Ping(ctx) != nil {
				return
			}
		}
	}
}

// termIn passes the seat holder's keys and size to the session and drops a watcher's. The first key
// reopens a done task before it is passed on, and a task that cannot be steered gets none.
func (s *server) termIn(ctx context.Context, c *websocket.Conn, st io.Writer, id domain.TaskID, v *viewer) {
	steered := false
	for {
		typ, b, err := c.Read(ctx)
		if err != nil {
			return
		}
		if s.seats.steering(id, v) && s.pass(ctx, st, id, typ, b, &steered) != nil {
			return
		}
	}
}

// pass gives the session one message from the seat holder: a size, or keys once the task took a steer.
func (s *server) pass(ctx context.Context, st io.Writer, id domain.TaskID, typ websocket.MessageType, b []byte, steered *bool) error {
	if typ == websocket.MessageText {
		resize(st, b)
		return nil
	}
	if !*steered && s.steer(ctx, id) != nil {
		return nil
	}
	*steered = true
	return keysIn(st, b)
}

func resize(st io.Writer, b []byte) {
	var sz termSize
	if json.Unmarshal(b, &sz) != nil || sz.Cols < 1 || sz.Rows < 1 || sz.Cols > maxSide || sz.Rows > maxSide {
		return
	}
	_ = wire.WriteTerm(st, wire.TermResize, wire.Size{Cols: uint16(sz.Cols), Rows: uint16(sz.Rows)}.Bytes())
}

func keysIn(st io.Writer, b []byte) error {
	for len(b) > 0 {
		n := min(len(b), wire.MaxTermFrame)
		if err := wire.WriteTerm(st, wire.TermIn, b[:n]); err != nil {
			return err //nolint:wrapcheck // the caller only stops on it
		}
		b = b[n:]
	}
	return nil
}

// steer is typing to the task, which reopens a done one. A task that cannot take it is refused.
func (s *server) steer(ctx context.Context, id domain.TaskID) error {
	if _, err := s.deps.Act.Steer(ctx, id); err != nil {
		return err //nolint:wrapcheck // the callers branch on the domain error
	}
	return nil
}
