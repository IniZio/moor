package web

import (
	"context"
	"fmt"
	"log"
	"net/http"
	"time"

	"github.com/tmaxmax/go-sse"
)

// listFeed tells one client that the task list changed. The message carries no rows, only the stamp of the task
// table: the client asks the list route again with its own filters, so a filter menu is never touched.
type listFeed struct {
	s    *server
	sess *sse.Session
	last string
	next time.Time
}

func (f *listFeed) step(ctx context.Context) error {
	stamp, err := f.s.deps.Tasks.TaskStamp(ctx)
	if err != nil {
		return fmt.Errorf("task stamp: %w", err)
	}
	now := f.s.now()
	if stamp == f.last && now.Before(f.next) {
		return nil
	}
	f.last, f.next = stamp, now.Add(ageEvery)
	m := &sse.Message{Type: sse.Type("tasks")}
	m.AppendData(stamp)
	return send(f.sess, m)
}

func (f *listFeed) idle(context.Context) error { return comment(f.sess) }

// taskListEvents serves GET /api/tasks/events. since is the stamp the page was drawn with, so a change between
// drawing the page and opening the stream is told at once. A stream also speaks every minute, so ages move.
func (s *server) taskListEvents(w http.ResponseWriter, r *http.Request) {
	sess, err := openStream(w, r)
	if err != nil {
		serverError(w, r, err)
		return
	}
	f := &listFeed{s: s, sess: sess, last: r.URL.Query().Get("since"), next: s.now().Add(ageEvery)}
	if err := pump(r.Context(), f.step, f.idle); err != nil {
		log.Printf("task list events: %v", err)
	}
}
