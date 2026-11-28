package web

import (
	"context"
	"errors"
	"fmt"
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// TaskActions is what a person does to a task from its Run page.
type TaskActions interface {
	Cancel(ctx context.Context, id domain.TaskID) (task.Outcome, error)
	End(ctx context.Context, id domain.TaskID) (task.Outcome, error)
	Retry(ctx context.Context, newID, oldID domain.TaskID) (task.Outcome, error)
}

// act runs an action on the task the path names. A task the action cannot change gets a 409 and the words in
// refused, which the page shows as a toast.
func (s *server) act(w http.ResponseWriter, r *http.Request, do func(context.Context, domain.TaskID) (task.Outcome, error), done, refused func(task.Outcome) string) {
	id := domain.TaskID(chi.URLParam(r, "id"))
	out, err := do(r.Context(), id)
	switch {
	case errors.Is(err, domain.ErrNotFound):
		http.NotFound(w, r)
	case errors.Is(err, domain.ErrRefused):
		reply(w, r, http.StatusConflict, refused(out), out)
	case err != nil:
		serverError(w, r, err)
	default:
		reply(w, r, http.StatusOK, done(out), out)
	}
}

// reply answers a page with a toast and anything else with the task in JSON.
func reply(w http.ResponseWriter, r *http.Request, status int, msg string, out task.Outcome) {
	if !isHTMX(r) {
		body := map[string]any{"task": jsonTaskOf(domain.TaskListing{Task: out.Task})}
		if status != http.StatusOK {
			body = map[string]any{"error": msg}
		}
		writeJSON(w, status, body)
		return
	}
	w.Header().Set("HX-Reswap", "none")
	render(w, r, status, toastResponse(msg))
}

func (s *server) stopTask(w http.ResponseWriter, r *http.Request) {
	s.act(w, r, s.deps.Act.Cancel,
		func(task.Outcome) string { return "Cancelled." },
		func(task.Outcome) string { return "The task has already finished." })
}

func (s *server) endTask(w http.ResponseWriter, r *http.Request) {
	s.act(w, r, s.deps.Act.End,
		func(task.Outcome) string { return "Ended. The task is done." },
		func(task.Outcome) string { return "Only a running task that a person started can be ended." })
}

func (s *server) retryTask(w http.ResponseWriter, r *http.Request) {
	old := domain.TaskID(chi.URLParam(r, "id"))
	id, err := s.deps.Tasks.NextTaskID(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	s.act(w, r, func(ctx context.Context, _ domain.TaskID) (task.Outcome, error) {
		return s.deps.Act.Retry(ctx, id, old)
	},
		func(out task.Outcome) string {
			return fmt.Sprintf("Started task #%s with the same prompt and repository.", out.Task.ID)
		},
		func(task.Outcome) string { return "Only a failed task that a person started can be re-run." })
}
