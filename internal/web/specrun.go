package web

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// SpecRunner starts the tasks of a task spec by hand. Input is what the spec's events need: a pull request
// link for a spec with a pull request event.
type SpecRunner interface {
	Run(ctx context.Context, spec, input string, by domain.Person) (task.Outcome, error)
}

const msgBadLink = "Paste a pull request link, such as https://github.com/example-org/shop-web/pull/91."

// runRefusal words why a Run was refused, with the status that goes with it. It gives 0 for an error that is not a refusal.
func runRefusal(err error) (string, int) {
	var denied *domain.NotAllowedError
	switch {
	case errors.Is(err, domain.ErrBadLink):
		return msgBadLink, http.StatusUnprocessableEntity
	case errors.As(err, &denied):
		return notAllowed(denied.Repo.Path), http.StatusUnprocessableEntity
	case errors.Is(err, domain.ErrNoSpec):
		return "This spec is not in the team's home any more.", http.StatusNotFound
	case errors.Is(err, domain.ErrNotRunnable):
		return "This spec cannot be started by hand.", http.StatusConflict
	case errors.Is(err, domain.ErrRunUnsupported):
		return "This portal cannot start that kind of Run yet.", http.StatusUnprocessableEntity
	}
	return "", 0
}

// readRun reads what a person typed into the Run form, from a JSON body or from the form htmx sends.
func readRun(w http.ResponseWriter, r *http.Request) (string, bool) {
	r.Body = http.MaxBytesReader(w, r.Body, maxBody)
	if strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
		var in struct {
			Input string `json:"input"`
		}
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			http.Error(w, "the body is not the JSON of a Run, an input", http.StatusBadRequest)
			return "", false
		}
		return in.Input, true
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "the form could not be read", http.StatusBadRequest)
		return "", false
	}
	return r.PostForm.Get("input"), true
}

// specNameOf reads the spec's name from the path, which chi leaves escaped when the request had its own escapes.
func specNameOf(r *http.Request) string {
	name := chi.URLParam(r, "name")
	if r.URL.RawPath != "" {
		if plain, err := url.PathUnescape(name); err == nil {
			return plain
		}
	}
	return name
}

func (s *server) runSpec(w http.ResponseWriter, r *http.Request) {
	input, ok := readRun(w, r)
	if !ok {
		return
	}
	if s.deps.Run == nil {
		http.Error(w, "this portal starts no task spec by hand", http.StatusNotFound)
		return
	}
	out, err := s.deps.Run.Run(r.Context(), specNameOf(r), input, domain.Person(personOf(r.Context())))
	if msg, status := runRefusal(err); status != 0 {
		reply(w, r, status, msg, task.Outcome{})
		return
	}
	if err != nil {
		serverError(w, r, err)
		return
	}
	if isHTMX(r) {
		w.Header().Set("HX-Trigger", "task-started")
	}
	reply(w, r, http.StatusOK, fmt.Sprintf("Started task #%s on %s.", out.Task.ID, out.Task.Repo.Path), out)
}
