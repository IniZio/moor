package main

import (
	"context"
	"errors"
	"fmt"
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/answer"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/inbound"
	"github.com/IniZio/moor/internal/task"
)

// githubApp is the GitHub App the portal acts as, or nothing when the portal has none.
type githubApp struct{ adapter *github.Adapter }

func newGitHub(cfg config) (githubApp, error) {
	if !cfg.github.Enabled() {
		return githubApp{}, nil
	}
	a, err := github.New(cfg.github)
	if err != nil {
		return githubApp{}, fmt.Errorf("github app: %w", err)
	}
	return githubApp{a}, nil
}

// taskConfig names the sources whose tasks answer their event. GitHub does only when the portal has its App.
func taskConfig(g githubApp) task.Config {
	if g.adapter == nil {
		return task.Config{}
	}
	return task.Config{Answerable: map[domain.EventSource]bool{github.Source: true}}
}

// readers reads the pull request events of tasks for the screens' titles.
func (g githubApp) readers() domain.EventReaders {
	if g.adapter == nil {
		return nil
	}
	return domain.EventReaders{github.Source: g.adapter}
}

// links reads the pull request links that Run is given.
func (g githubApp) links() inbound.Linker {
	if g.adapter == nil {
		return noLinks{}
	}
	return links{g.adapter}
}

// mount serves GitHub's webhook beside next, authenticated by its signature and by no person.
func (g githubApp) mount(next http.Handler, events *inbound.Service) http.Handler {
	if g.adapter == nil {
		return next
	}
	r := chi.NewRouter()
	r.Method(http.MethodPost, "/hooks/github", g.adapter.Hook(events))
	r.Mount("/", next)
	return r
}

// answers makes the service that posts a published result back as the one comment of its pull request.
func (g githubApp) answers(db *store.Store, tasks *task.Service) *answer.Service {
	posters := map[domain.EventSource]answer.Poster{}
	if g.adapter != nil {
		posters[github.Source] = g.adapter
	}
	return answer.New(answer.Deps{Tasks: tasks, Reads: db, Deliveries: db, Posters: posters, Clock: systemClock{}})
}

type links struct{ a *github.Adapter }

func (l links) Occurrence(ctx context.Context, link string) (domain.Occurrence, error) {
	occ, err := l.a.Occurrence(ctx, link)
	if errors.Is(err, github.ErrNotPullRequestLink) {
		return occ, fmt.Errorf("%w: %w", domain.ErrBadLink, err)
	}
	if err != nil {
		return occ, fmt.Errorf("github: %w", err)
	}
	return occ, nil
}

// noLinks stands in when the portal has no GitHub App, so no link can be read.
type noLinks struct{}

func (noLinks) Occurrence(context.Context, string) (domain.Occurrence, error) {
	return domain.Occurrence{}, fmt.Errorf("%w: the portal has no GitHub App", domain.ErrRunUnsupported)
}

// txStore lends the store's transactions to the inbound service.
type txStore struct{ db *store.Store }

func (s txStore) InTx(ctx context.Context, fn func(inbound.Tx) error) error {
	if err := s.db.InTx(ctx, func(tx *store.Store) error { return fn(tx) }); err != nil {
		return fmt.Errorf("transaction: %w", err)
	}
	return nil
}
