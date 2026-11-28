package web

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/tmaxmax/go-sse"

	"github.com/IniZio/moor/internal/domain"
)

// jsonRow is an event row as the events stream and the snapshot carry it.
type jsonRow struct {
	Seq  int64            `json:"seq"`
	At   time.Time        `json:"at"`
	Kind domain.EventKind `json:"kind"`
	Data json.RawMessage  `json:"data"`
}

func jsonRowOf(r domain.EventRow) jsonRow {
	data := r.Data
	if len(data) == 0 {
		data = json.RawMessage("null")
	}
	return jsonRow{Seq: r.Seq, At: r.At, Kind: r.Kind, Data: data}
}

// eventsAfter reads every row of a task above seq, a page at a time.
func (s *server) eventsAfter(ctx context.Context, id domain.TaskID, seq int64) ([]domain.EventRow, error) {
	var all []domain.EventRow
	for {
		rows, err := s.deps.Tasks.EventsAfter(ctx, id, seq, rowsPerRead)
		if err != nil {
			return nil, fmt.Errorf("events of %s: %w", id, err)
		}
		all = append(all, rows...)
		if len(rows) < rowsPerRead {
			return all, nil
		}
		seq = rows[len(rows)-1].Seq
	}
}

// resumeAfter is the seq a stream continues from: the one the client names in Last-Event-ID after a drop, or
// else the one in the query that the page was drawn with. A query that is not a seq is refused.
func resumeAfter(header, q string) (int64, error) {
	if n, err := strconv.ParseInt(header, 10, 64); err == nil && n >= 0 {
		return n, nil
	}
	if q == "" {
		return 0, nil
	}
	n, err := strconv.ParseInt(q, 10, 64)
	if err != nil || n < 0 {
		return 0, errors.New("after is a seq, such as 17")
	}
	return n, nil
}

// runFeed follows one task's event rows for one client. In page mode each batch of new rows is one message, the
// page drawn from every row so far, with the id of the batch's last row, so a replayed or repeated message draws
// the same page. In row mode each row is one message with its own seq as its id.
type runFeed struct {
	s    *server
	id   domain.TaskID
	sess *sse.Session
	// seen is the last seq read. In page mode sent is the last seq the client already has.
	seen, sent int64
	rows       []domain.EventRow
	page       bool
	part       string
}

func (f *runFeed) step(ctx context.Context) error {
	fresh, err := f.s.eventsAfter(ctx, f.id, f.seen)
	if err != nil || len(fresh) == 0 {
		return err
	}
	f.seen = fresh[len(fresh)-1].Seq
	if !f.page {
		return f.sendRows(fresh)
	}
	f.rows = append(f.rows, fresh...)
	if f.seen <= f.sent {
		return nil
	}
	return f.sendPage(ctx, f.seen)
}

func (f *runFeed) sendRows(rows []domain.EventRow) error {
	for _, r := range rows {
		raw, err := json.Marshal(jsonRowOf(r))
		if err != nil {
			return fmt.Errorf("encode row %d: %w", r.Seq, err)
		}
		m := &sse.Message{ID: sse.ID(strconv.FormatInt(r.Seq, 10)), Type: sse.Type("row")}
		m.AppendData(string(raw))
		if err := send(f.sess, m); err != nil {
			return err
		}
	}
	return nil
}

// sendPage draws the page from the task as it is now and the rows so far. A zero seq sends it with no id, for the
// refresh that keeps the times of a running task moving.
func (f *runFeed) sendPage(ctx context.Context, seq int64) error {
	t, err := f.s.deps.Tasks.Load(ctx, f.id)
	if err != nil {
		return fmt.Errorf("load task %s: %w", f.id, err)
	}
	var html strings.Builder
	if err := runBody(buildRun(t, f.rows, f.s.now()), f.part).Render(ctx, &html); err != nil {
		return fmt.Errorf("draw the run page: %w", err)
	}
	m := &sse.Message{Type: sse.Type("page")}
	if seq > 0 {
		m.ID = sse.ID(strconv.FormatInt(seq, 10))
	}
	m.AppendData(html.String())
	return send(f.sess, m)
}

// idle redraws the page of a task that is still live, so its times move, and otherwise only keeps the stream open.
func (f *runFeed) idle(ctx context.Context) error {
	if f.page {
		t, err := f.s.deps.Tasks.Load(ctx, f.id)
		if err != nil {
			return fmt.Errorf("load task %s: %w", f.id, err)
		}
		if t.State.Live() {
			return f.sendPage(ctx, 0)
		}
	}
	return comment(f.sess)
}

// runEvents serves GET /api/tasks/{id}/events. The rows after the seq in Last-Event-ID, or in after, come from
// Postgres by seq, so any replica gives the same stream. With view=page each message is the drawn Run page.
func (s *server) runEvents(w http.ResponseWriter, r *http.Request) {
	id := domain.TaskID(chi.URLParam(r, "id"))
	q := r.URL.Query()
	after, err := resumeAfter(r.Header.Get("Last-Event-ID"), q.Get("after"))
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if _, err := s.deps.Tasks.Load(r.Context(), id); err != nil {
		s.taskError(w, r, err)
		return
	}
	sess, err := openStream(w, r)
	if err != nil {
		serverError(w, r, err)
		return
	}
	f := &runFeed{s: s, id: id, sess: sess, seen: after, sent: after, page: q.Get("view") == "page", part: q.Get("part")}
	if f.page {
		f.seen = 0
	}
	if err := pump(r.Context(), f.step, f.idle); err != nil {
		log.Printf("events of task %s: %v", id, err)
	}
}

func (s *server) taskError(w http.ResponseWriter, r *http.Request, err error) {
	if errors.Is(err, domain.ErrNotFound) {
		http.NotFound(w, r)
		return
	}
	serverError(w, r, err)
}
