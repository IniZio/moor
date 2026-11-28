package web

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"slices"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// SpecSource lists the task specs of the team's home.
type SpecSource interface {
	TaskSpecs(ctx context.Context) ([]domain.SpecFile, error)
}

// specEntry is a task spec file as the screens show it. Name is empty when the file holds no spec, and
// Problem says why a file is not usable, whether it did not parse or names a repository Settings lacks.
type specEntry struct {
	File    string
	Name    string
	Spec    domain.TaskSpec
	Problem string
}

// keyOf picks the entry in the left list: the spec's name, or the file when the file holds no spec.
func (e specEntry) keyOf() string {
	if e.Name != "" {
		return e.Name
	}
	return e.File
}

func (s *server) specEntries(ctx context.Context, allowed []domain.Repo) ([]specEntry, error) {
	files, err := s.deps.Specs.TaskSpecs(ctx)
	if err != nil {
		return nil, fmt.Errorf("task specs: %w", err)
	}
	out := make([]specEntry, len(files))
	for i, f := range files {
		out[i] = specEntry{File: f.File, Name: string(f.Spec.Name), Spec: f.Spec, Problem: f.Problem}
		if !f.Valid() {
			continue
		}
		var missing []string
		for _, r := range domain.RepositoriesNotAllowed(f.Spec, domain.Settings{Repositories: allowed}) {
			missing = append(missing, string(r.Path))
		}
		if len(missing) > 0 {
			out[i].Problem = strings.Join(missing, ", ") + " is not under repositories in Settings. Add it there first."
		}
	}
	return out, nil
}

// startsText says when a spec starts a task, read from its on.
func startsText(s domain.TaskSpec) string {
	var when []string
	if pr := s.On.PullRequest; pr != nil {
		when = append(when, "when a pull request is "+pullRequestVerbs(pr.Types)+" on "+repositoryScope(s))
	}
	if len(s.On.Schedule) > 0 {
		crons := make([]string, len(s.On.Schedule))
		for i, sc := range s.On.Schedule {
			crons[i] = sc.Cron
		}
		when = append(when, "on the schedule "+strings.Join(crons, " and ")+", one task per repository")
	}
	switch {
	case len(when) == 0:
		return "Starts only when it is run by hand."
	case s.On.WorkflowDispatch != nil:
		return "Starts " + strings.Join(when, ", and ") + ". You can also start it by hand."
	}
	return "Starts " + strings.Join(when, ", and ") + "."
}

func repositoryScope(s domain.TaskSpec) string {
	if len(s.Repositories) == 0 {
		return "any repository in Settings"
	}
	paths := make([]string, len(s.Repositories))
	for i, r := range s.Repositories {
		paths[i] = string(r.Path)
	}
	return strings.Join(paths, " or ")
}

// pullRequestVerbs words the activity types a spec listens to: opened, updated and closed.
func pullRequestVerbs(types []domain.PullRequestType) string {
	if len(types) == 0 {
		types = domain.DefaultPullRequestTypes
	}
	var verbs []string
	add := func(v string) {
		if !slices.Contains(verbs, v) {
			verbs = append(verbs, v)
		}
	}
	for _, t := range types {
		switch t {
		case domain.PROpened, domain.PRReopened, domain.PRReadyForReview:
			add("opened")
		case domain.PRSynchronize:
			add("updated")
		case domain.PRClosed:
			add("closed")
		}
	}
	return strings.Join(verbs, " or ")
}

// runLabel names what a Run asks for: a pull request link for a spec with a pull request event, otherwise a repository.
func runLabel(s domain.TaskSpec) string {
	if s.On.PullRequest != nil {
		return "Pull request link"
	}
	return "Repository"
}

func runPlaceholder(s domain.TaskSpec) string {
	if s.On.PullRequest != nil {
		return "https://github.com/example-org/shop-web/pull/91"
	}
	return "example-org/shop-web"
}

func runURL(e specEntry) string { return "/api/task-specs/" + url.PathEscape(e.Name) + "/run" }

type jsonSpec struct {
	File         string   `json:"file"`
	Name         string   `json:"name"`
	Starts       string   `json:"starts"`
	Manual       bool     `json:"manual"`
	Repositories []string `json:"repositories"`
	Problem      string   `json:"problem"`
}

func (s *server) taskSpecs(w http.ResponseWriter, r *http.Request) {
	allowed, err := s.deps.Settings.Allowed(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	entries, err := s.specEntries(r.Context(), allowed)
	if err != nil {
		serverError(w, r, err)
		return
	}
	out := make([]jsonSpec, len(entries))
	for i, e := range entries {
		out[i] = jsonSpec{File: e.File, Name: e.Name, Manual: e.Spec.On.WorkflowDispatch != nil, Repositories: []string{}, Problem: e.Problem}
		if e.Name != "" {
			out[i].Starts = startsText(e.Spec)
		}
		for _, repo := range e.Spec.Repositories {
			out[i].Repositories = append(out[i].Repositories, string(repo.Path))
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{"specs": out})
}
