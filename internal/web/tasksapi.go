package web

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// TaskStarter starts tasks for people.
type TaskStarter interface {
	StartByPerson(ctx context.Context, in task.StartInput) (task.Outcome, error)
}

// TaskReader reads tasks and hands out their numbers.
type TaskReader interface {
	ListTasks(ctx context.Context, f domain.TaskFilter) (domain.TaskList, error)
	// Starters lists the people who started tasks in the scope of f, which is its Spec and FromPrompt.
	Starters(ctx context.Context, f domain.TaskFilter) ([]string, error)
	NextTaskID(ctx context.Context) (domain.TaskID, error)
	Load(ctx context.Context, id domain.TaskID) (domain.Task, error)
	Versions(ctx context.Context, id domain.TaskID) ([]domain.ResultVersion, error)
	// EventsAfter lists up to limit event rows of a task above seq, in seq order.
	EventsAfter(ctx context.Context, id domain.TaskID, after int64, limit int) ([]domain.EventRow, error)
	// TaskStamp changes whenever any task is created or written.
	TaskStamp(ctx context.Context) (string, error)
}

const listLimit = 200

const msgEmptyPrompt = "Write what the agent should do first."

func notAllowed(path domain.RepoPath) string {
	return string(path) + " is not under repositories in Settings. Add it there first."
}

type jsonRepo struct {
	Host string `json:"host"`
	Path string `json:"path"`
}

type jsonTask struct {
	ID         string     `json:"id"`
	Title      string     `json:"title"`
	State      string     `json:"state"`
	Group      string     `json:"group"`
	Repository jsonRepo   `json:"repository"`
	Spec       string     `json:"spec"`
	StartedBy  string     `json:"started_by"`
	ReplacedBy string     `json:"replaced_by,omitempty"`
	CreatedAt  time.Time  `json:"created_at"`
	EndedAt    *time.Time `json:"ended_at"`
}

func jsonTaskOf(it domain.TaskListing) jsonTask {
	t := it.Task
	out := jsonTask{
		ID: string(t.ID), Title: taskTitle(t), State: t.State.String(), Group: string(t.State.Group()),
		Repository: jsonRepo{Host: string(t.Repo.Host), Path: string(t.Repo.Path)},
		StartedBy:  t.StartedBy, ReplacedBy: string(it.ReplacedBy), CreatedAt: t.CreatedAt,
	}
	if t.Spec != nil {
		out.Spec = string(t.Spec.Name)
	}
	if !t.EndedAt.IsZero() {
		out.EndedAt = &t.EndedAt
	}
	return out
}

// parseFilter reads the query of GET /api/tasks. A value left empty narrows nothing, as a form sends it.
func parseFilter(q url.Values) (domain.TaskFilter, error) {
	f := domain.TaskFilter{Spec: domain.SpecName(q.Get("spec")), StartedBy: q.Get("started_by"), Limit: listLimit}
	switch q.Get("from") {
	case "":
	case "prompt":
		f.FromPrompt = true
	default:
		return f, errors.New("from is prompt, or left out")
	}
	if f.StartedBy == domain.EventStarterPrefix {
		f.StartedBy, f.ByEvent = "", true
	}
	if text := q.Get("repository"); text != "" {
		repo, err := domain.ParseRepo(text)
		if err != nil {
			return f, fmt.Errorf("repository: %w", err)
		}
		f.Repo = repo
	}
	if text := q.Get("state"); text != "" {
		group, err := domain.ParseStateGroup(text)
		if err != nil {
			return f, fmt.Errorf("state: %w", err)
		}
		f.Group = group
	}
	return f, nil
}

func (s *server) listTasks(w http.ResponseWriter, r *http.Request) {
	f, err := parseFilter(r.URL.Query())
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	if isHTMX(r) {
		s.listFragment(w, r, f)
		return
	}
	list, err := s.deps.Tasks.ListTasks(r.Context(), f)
	if err != nil {
		serverError(w, r, err)
		return
	}
	out := make([]jsonTask, len(list.Items))
	for i, it := range list.Items {
		out[i] = s.jsonTaskRead(it)
	}
	writeJSON(w, http.StatusOK, map[string]any{"tasks": out, "more": list.More})
}

type startBody struct {
	Repository string `json:"repository"`
	Prompt     string `json:"prompt"`
}

// readStart reads the repository and prompt a person gave, from a JSON body or from the form htmx sends.
func readStart(w http.ResponseWriter, r *http.Request) (startBody, bool) {
	r.Body = http.MaxBytesReader(w, r.Body, maxBody)
	var in startBody
	if strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			http.Error(w, "the body is not the JSON of a new task, repository and prompt", http.StatusBadRequest)
			return in, false
		}
		return in, true
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "the form could not be read", http.StatusBadRequest)
		return in, false
	}
	return startBody{Repository: r.PostForm.Get("repository"), Prompt: r.PostForm.Get("prompt")}, true
}

// startRepo checks the repository a person chose against the newest settings, and says why not in the words
// a person reads. The empty text and the error are both empty when the repository is allowed.
func (s *server) startRepo(ctx context.Context, text string) (domain.Repo, string, error) {
	repo, err := domain.ParseRepo(text)
	if err != nil {
		return repo, fmt.Sprintf("%q is not a repository, such as example-org/shop-web.", text), nil
	}
	allowed, err := s.deps.Settings.Allowed(ctx)
	if err != nil {
		return repo, "", fmt.Errorf("allowed repositories: %w", err)
	}
	if !slices.Contains(allowed, repo) {
		return repo, notAllowed(repo.Path), nil
	}
	return repo, "", nil
}

func (s *server) startTask(w http.ResponseWriter, r *http.Request) {
	in, ok := readStart(w, r)
	if !ok {
		return
	}
	prompt := strings.TrimSpace(in.Prompt)
	if prompt == "" {
		refuseStart(w, r, msgEmptyPrompt)
		return
	}
	repo, why, err := s.startRepo(r.Context(), strings.TrimSpace(in.Repository))
	switch {
	case err != nil:
		serverError(w, r, err)
		return
	case why != "":
		refuseStart(w, r, why)
		return
	}
	id, err := s.deps.Tasks.NextTaskID(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	out, err := s.deps.Start.StartByPerson(r.Context(), task.StartInput{ID: id, Repo: repo, Prompt: prompt, By: domain.Person(personOf(r.Context()))})
	if err != nil {
		serverError(w, r, err)
		return
	}
	if !isHTMX(r) {
		writeJSON(w, http.StatusCreated, map[string]any{"task": jsonTaskOf(domain.TaskListing{Task: out.Task})})
		return
	}
	w.Header().Set("HX-Trigger", "task-started")
	render(w, r, http.StatusOK, startedResponse(fmt.Sprintf("Started task #%s on %s.", out.Task.ID, repo.Path), s.allowedRepos(r)))
}

// allowedRepos lists the repositories for a fresh task box, and none when settings cannot be read.
func (s *server) allowedRepos(r *http.Request) []domain.Repo {
	repos, err := s.deps.Settings.Allowed(r.Context())
	if err != nil {
		log.Printf("allowed repositories: %v", err)
	}
	return repos
}

func refuseStart(w http.ResponseWriter, r *http.Request, msg string) {
	if !isHTMX(r) {
		writeJSON(w, http.StatusUnprocessableEntity, map[string]any{"error": msg})
		return
	}
	w.Header().Set("HX-Reswap", "none")
	render(w, r, http.StatusUnprocessableEntity, toastResponse(msg))
}
