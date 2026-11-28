package web

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"strconv"
	"strings"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// MaxPageBytes caps one published page. A page is one file, so its screenshots travel inside it.
const MaxPageBytes = 8 << 20

// ResultInput is one call of the desk's publish tool: the page, the one line that says how it went, and the key of
// that run of the tool. The same key sent again is the same publish, so a call the tool repeats after a failure makes
// no second version.
type ResultInput struct {
	Line string `json:"line"`
	Page string `json:"page"`
	Key  string `json:"key"`
}

// ResultReceipt is what a publish made: the version, where its page is served and the state the task is in.
type ResultReceipt struct {
	Version int    `json:"version"`
	URL     string `json:"url"`
	State   string `json:"state"`
}

// Results takes the pages the desk publishes and serves them. Publish fails with domain.ErrBadToken for a token that
// is not the task's task-scope token.
type Results interface {
	Publish(ctx context.Context, id domain.TaskID, token string, in ResultInput) (ResultReceipt, error)
	Page(ctx context.Context, id domain.TaskID, version int) ([]byte, error)
}

// resultRoutes serves the publish route, which the desk calls with its task's token and no person, and the pages,
// which a signed-in person reads.
func (s *server) resultRoutes(r chi.Router) {
	if s.deps.Results == nil {
		return
	}
	r.Post("/api/tasks/{id}/results", s.publishResult)
	r.Group(func(r chi.Router) {
		r.Use(s.person)
		r.Get("/results/{id}/{n}", s.resultPage)
	})
}

// maxPublishBody leaves room for the page's escapes in the JSON around it.
const maxPublishBody = 3 * MaxPageBytes

func (s *server) publishResult(w http.ResponseWriter, r *http.Request) {
	var in ResultInput
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxPublishBody))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&in); err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			http.Error(w, "the page is over its size cap", http.StatusRequestEntityTooLarge)
			return
		}
		http.Error(w, "the body is not a result: line, page and key", http.StatusBadRequest)
		return
	}
	if status, msg := checkResult(in); status != 0 {
		http.Error(w, msg, status)
		return
	}
	got, err := s.deps.Results.Publish(r.Context(), domain.TaskID(chi.URLParam(r, "id")), bearer(r), in)
	if err != nil {
		resultError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, got)
}

// maxLine caps the one line of a result, which the pull request comment and the Run page show whole.
const maxLine = 500

// checkResult says how a result is wrong, or gives status 0.
func checkResult(in ResultInput) (status int, msg string) {
	switch {
	case len(in.Page) > MaxPageBytes:
		return http.StatusRequestEntityTooLarge, "the page is over its size cap of " + strconv.Itoa(MaxPageBytes) + " bytes"
	case in.Page == "" || strings.TrimSpace(in.Line) == "":
		return http.StatusBadRequest, "a result needs a page and one line that says how it went"
	case len(in.Line) > maxLine || strings.ContainsAny(in.Line, "\r\n"):
		return http.StatusBadRequest, "the line is one line of at most " + strconv.Itoa(maxLine) + " bytes"
	}
	return 0, ""
}

func resultError(w http.ResponseWriter, r *http.Request, err error) {
	switch {
	case errors.Is(err, domain.ErrBadToken):
		http.Error(w, "this route takes the task's own token", http.StatusUnauthorized)
	case errors.Is(err, task.ErrBadInput):
		http.Error(w, err.Error(), http.StatusBadRequest)
	case errors.Is(err, domain.ErrRefused):
		http.Error(w, "the task cannot take a result now: "+err.Error(), http.StatusConflict)
	case errors.Is(err, domain.ErrNotFound):
		http.Error(w, "no such task", http.StatusNotFound)
	default:
		serverError(w, r, err)
	}
}

// pageSandbox lets a page run its own scripts and nothing else. It has no origin of its own to reach the portal's
// cookies or routes with, and no network to send what it reads to.
const pageSandbox = "sandbox allow-scripts allow-popups; default-src 'none'; img-src data: blob:; media-src data: blob:; " +
	"font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"

func (s *server) resultPage(w http.ResponseWriter, r *http.Request) {
	n, err := strconv.Atoi(chi.URLParam(r, "n"))
	if err != nil || strings.HasPrefix(chi.URLParam(r, "n"), "+") {
		http.NotFound(w, r)
		return
	}
	page, err := s.deps.Results.Page(r.Context(), domain.TaskID(chi.URLParam(r, "id")), n)
	if errors.Is(err, domain.ErrNotFound) {
		http.NotFound(w, r)
		return
	}
	if err != nil {
		serverError(w, r, err)
		return
	}
	h := w.Header()
	h.Set("Content-Type", "text/html; charset=utf-8")
	h.Set("Content-Security-Policy", pageSandbox)
	h.Set("X-Content-Type-Options", "nosniff")
	h.Set("Referrer-Policy", "no-referrer")
	h.Set("Cache-Control", "private, max-age=3600")
	_, _ = w.Write(page)
}
