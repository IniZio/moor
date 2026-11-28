package web

import (
	"context"
	"fmt"
	"log"
	"net/http"
	"time"

	"github.com/tmaxmax/go-sse"
)

// stampFeed tells one client that a list changed. The message carries no rows, only a stamp of what the list
// shows: the client asks the list route again with its own filters, so a filter menu is never touched. every
// is how often an unchanged list is told again, so ages move, and zero never repeats it.
type stampFeed struct {
	sess  *sse.Session
	event sse.EventType
	stamp func(context.Context) (string, error)
	every time.Duration
	now   func() time.Time
	last  string
	next  time.Time
}

func (s *server) newFeed(sess *sse.Session, event string, since string, every time.Duration, stamp func(context.Context) (string, error)) *stampFeed {
	return &stampFeed{sess: sess, event: sse.Type(event), stamp: stamp, every: every, now: s.now, last: since, next: s.now().Add(every)}
}

func (f *stampFeed) step(ctx context.Context) error {
	stamp, err := f.stamp(ctx)
	if err != nil {
		return fmt.Errorf("stamp: %w", err)
	}
	now := f.now()
	if stamp == f.last && (f.every == 0 || now.Before(f.next)) {
		return nil
	}
	f.last, f.next = stamp, now.Add(f.every)
	m := &sse.Message{Type: f.event}
	m.AppendData(stamp)
	return send(f.sess, m)
}

func (f *stampFeed) idle(context.Context) error { return comment(f.sess) }

// run speaks until the client leaves. A client leaving is not a failure, and any other end is logged.
func (f *stampFeed) run(ctx context.Context, what string) {
	if err := pump(ctx, f.step, f.idle); err != nil {
		log.Printf("%s: %v", what, err)
	}
}

// taskListEvents serves GET /api/tasks/events. since is the stamp the page was drawn with, so a change between
// drawing the page and opening the stream is told at once. A stream also speaks every minute, so ages move.
func (s *server) taskListEvents(w http.ResponseWriter, r *http.Request) {
	sess, err := openStream(w, r)
	if err != nil {
		serverError(w, r, err)
		return
	}
	s.newFeed(sess, "tasks", r.URL.Query().Get("since"), ageEvery, s.deps.Tasks.TaskStamp).run(r.Context(), "task list events")
}
