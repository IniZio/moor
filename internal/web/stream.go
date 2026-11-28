package web

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"time"

	"github.com/tmaxmax/go-sse"
)

// The pace of a stream. A stream reads the database every pollEvery, which is the path that always works, and
// speaks every keepAliveEvery so that a proxy keeps it open. The task list redraws its ages every ageEvery.
const (
	pollEvery      = time.Second
	keepAliveEvery = 15 * time.Second
	ageEvery       = time.Minute
	retryAfter     = 3 * time.Second
	rowsPerRead    = 500
)

// openStream turns the request into a server-sent event stream and sends the first bytes at once, so the client
// sees it open. A stream lives as long as the page, so the server's write deadline is lifted for it. go-sse frames
// the events and reads the Last-Event-ID the client sends again after a drop.
func openStream(w http.ResponseWriter, r *http.Request) (*sse.Session, error) {
	if err := http.NewResponseController(w).SetWriteDeadline(time.Time{}); err != nil && !errors.Is(err, http.ErrNotSupported) {
		return nil, fmt.Errorf("lift the write deadline: %w", err)
	}
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("X-Accel-Buffering", "no")
	sess, err := sse.Upgrade(w, r)
	if err != nil {
		return nil, fmt.Errorf("upgrade to a stream: %w", err)
	}
	hello := &sse.Message{Retry: retryAfter}
	hello.AppendComment("open")
	if err := send(sess, hello); err != nil {
		return nil, err
	}
	return sess, nil
}

func send(sess *sse.Session, m *sse.Message) error {
	if err := sess.Send(m); err != nil {
		return fmt.Errorf("send: %w", err)
	}
	if err := sess.Flush(); err != nil {
		return fmt.Errorf("flush: %w", err)
	}
	return nil
}

func comment(sess *sse.Session) error {
	m := &sse.Message{}
	m.AppendComment("keep-alive")
	return send(sess, m)
}

// pump runs step at once and then every pollEvery, and idle every keepAliveEvery, until the client leaves, the
// server shuts down or one of them fails. The client leaving is not a failure.
func pump(ctx context.Context, step func(context.Context) error, idle func(context.Context) error) error {
	if err := step(ctx); err != nil {
		return err
	}
	poll, keep := time.NewTicker(pollEvery), time.NewTicker(keepAliveEvery)
	defer poll.Stop()
	defer keep.Stop()
	for {
		var err error
		select {
		case <-ctx.Done():
			return nil
		case <-poll.C:
			err = step(ctx)
		case <-keep.C:
			err = idle(ctx)
		}
		if err != nil {
			return err
		}
	}
}
