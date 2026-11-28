package web

import (
	"context"
	"fmt"
	"net/http"
	"slices"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
)

// loadRun reads the event rows first and the task after, so the task is never older than the rows: a write
// that lands between the two reads has its rows on the stream that the page opens after the last seq it drew.
func (s *server) loadRun(ctx context.Context, id domain.TaskID) (domain.Task, []domain.EventRow, error) {
	rows, err := s.eventsAfter(ctx, id, 0)
	if err != nil {
		return domain.Task{}, nil, err
	}
	t, err := s.deps.Tasks.Load(ctx, id)
	if err != nil {
		return domain.Task{}, nil, fmt.Errorf("load task %s: %w", id, err)
	}
	return t, rows, nil
}

// runPage serves a task's Summary, and a part when the path names one.
func (s *server) runPage(w http.ResponseWriter, r *http.Request) {
	part := chi.URLParam(r, "part")
	if part != "" && !slices.Contains(partKeys, part) {
		http.NotFound(w, r)
		return
	}
	t, rows, err := s.loadRun(r.Context(), domain.TaskID(chi.URLParam(r, "id")))
	if err != nil {
		s.taskError(w, r, err)
		return
	}
	v := s.run(t, rows)
	v.Blocked = blockedOf(s.blockedList(r.Context(), t.ID))
	v.Page = s.pageData(r)
	render(w, r, http.StatusOK, runPage(v.Page, v, part))
}

type jsonResultVersion struct {
	N           int       `json:"version"`
	EntryURL    string    `json:"entry_url"`
	Line        string    `json:"line"`
	PublishedAt time.Time `json:"published_at"`
}

type jsonDetail struct {
	jsonTask
	Prompt  string `json:"prompt"`
	Failure string `json:"failure"`
	Commit  string `json:"commit"`
	Runner  string `json:"runner"`
	Run     int64  `json:"run"`
	Gen     int64  `json:"gen"`
}

// taskSnapshot serves GET /api/tasks/{id}: the task, its result versions, its event rows and the last seq. The
// stream then opens with after set to that seq, and nothing falls in the gap.
func (s *server) taskSnapshot(w http.ResponseWriter, r *http.Request) {
	id := domain.TaskID(chi.URLParam(r, "id"))
	t, rows, err := s.loadRun(r.Context(), id)
	if err != nil {
		s.taskError(w, r, err)
		return
	}
	versions, err := s.deps.Tasks.Versions(r.Context(), id)
	if err != nil {
		serverError(w, r, err)
		return
	}
	out := struct {
		Task     jsonDetail          `json:"task"`
		Versions []jsonResultVersion `json:"versions"`
		Events   []jsonRow           `json:"events"`
		LastSeq  int64               `json:"last_seq"`
		Blocked  []jsonBlocked       `json:"blocked_requests"`
	}{
		Task:     jsonDetail{jsonTask: s.jsonTaskRead(domain.TaskListing{Task: t}), Prompt: t.Prompt, Failure: t.Failure, Commit: string(t.Commit), Runner: string(t.Runner), Run: t.Run, Gen: t.Gen},
		Versions: resultVersionsOf(versions), Events: make([]jsonRow, len(rows)),
		Blocked: jsonBlockedOf(s.blockedList(r.Context(), id)),
	}
	for i, row := range rows {
		out.Events[i] = jsonRowOf(row)
		out.LastSeq = row.Seq
	}
	writeJSON(w, http.StatusOK, out)
}

func resultVersionsOf(vs []domain.ResultVersion) []jsonResultVersion {
	out := make([]jsonResultVersion, len(vs))
	for i, v := range vs {
		out[i] = jsonResultVersion{N: v.N, EntryURL: v.EntryURL, Line: v.Line, PublishedAt: v.PublishedAt}
	}
	return out
}
