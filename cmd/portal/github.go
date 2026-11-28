package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/answer"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/inbound"
	"github.com/IniZio/moor/internal/settings"
	"github.com/IniZio/moor/internal/task"
)

// githubApp is the GitHub App the portal acts as, read from the settings' github_app credential at each use, so
// a save that adds, rotates or removes it applies at once. The adapter is kept while the App's keys stay the same.
type githubApp struct {
	svc *settings.Service
	mu  sync.Mutex
	key []byte
	a   *github.Adapter
	// pushes hears the pushes the webhook carries, and is nil when nothing wants them.
	pushes github.Pushes
}

// errNoApp is a GitHub call made while the settings hold no App.
var errNoApp = errors.New("the settings hold no GitHub App")

// adapter is the adapter of the settings' App now.
func (g *githubApp) adapter(ctx context.Context) (*github.Adapter, error) {
	keys, err := g.svc.GitHubApp(ctx)
	if errors.Is(err, settings.ErrNoApp) {
		return nil, errNoApp
	}
	if err != nil {
		return nil, fmt.Errorf("github app: %w", err)
	}
	return g.of(keys)
}

// of is the adapter of keys, the one already made when the keys are the same.
func (g *githubApp) of(keys domain.AppKeys) (*github.Adapter, error) {
	id := bytes.Join([][]byte{[]byte(strconv.FormatInt(keys.AppID, 10)), keys.PrivateKey, keys.WebhookSecret, []byte(keys.API())}, []byte{0})
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.a != nil && bytes.Equal(g.key, id) {
		return g.a, nil
	}
	a, err := github.New(github.Config{AppID: keys.AppID, Key: keys.PrivateKey, WebhookSecret: keys.WebhookSecret, APIBase: keys.API()})
	if err != nil {
		return nil, fmt.Errorf("github app: %w", err)
	}
	g.key, g.a = id, a
	return a, nil
}

// Mint is the settings service's minter: a token of the App's installation limited to the task's repository.
func (g *githubApp) Mint(ctx context.Context, keys domain.AppKeys, repo domain.Repo) (domain.CredentialValue, error) {
	a, err := g.of(keys)
	if err != nil {
		return domain.CredentialValue{}, err
	}
	ctx, cancel := context.WithTimeout(ctx, tokenTimeout)
	defer cancel()
	token, expires, err := a.TaskToken(ctx, string(repo.Path))
	if err != nil {
		return domain.CredentialValue{}, fmt.Errorf("github: %w", err)
	}
	return domain.CredentialValue{Value: token, Expires: expires}, nil
}

// defaultBranch reads the repository's default branch as the settings' App. It gives up after tokenTimeout.
func (g *githubApp) defaultBranch(ctx context.Context, repo domain.Repo) (string, error) {
	if repo.Host != domain.DefaultRepoHost {
		return "", fmt.Errorf("%s is not on GitHub", repo)
	}
	a, err := g.adapter(ctx)
	if err != nil {
		return "", err
	}
	ctx, cancel := context.WithTimeout(ctx, tokenTimeout)
	defer cancel()
	return a.DefaultBranch(ctx, string(repo.Path)) //nolint:wrapcheck // the adapter names the repository
}

// taskConfig names the sources whose tasks answer their event. A task GitHub started answers it.
func taskConfig() task.Config {
	return task.Config{Answerable: map[domain.EventSource]bool{github.Source: true}}
}

// readers reads the pull request events of tasks for the screens' titles, which needs no App.
func readers() domain.EventReaders { return domain.EventReaders{github.Source: &github.Adapter{}} }

// mount serves GitHub's webhook beside next, authenticated by its signature and by no person.
func (g *githubApp) mount(next http.Handler, events *inbound.Service) http.Handler {
	r := chi.NewRouter()
	r.Method(http.MethodPost, "/hooks/github", http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		a, err := g.adapter(req.Context())
		if err != nil {
			http.Error(w, "the portal has no GitHub App in its settings", http.StatusServiceUnavailable)
			return
		}
		a.HookFor(events, g.pushes).ServeHTTP(w, req)
	}))
	r.Mount("/", next)
	return r
}

// answers makes the service that posts a published result back as the one comment of its pull request.
func (g *githubApp) answers(db *store.Store, tasks *task.Service) *answer.Service {
	posters := map[domain.EventSource]answer.Poster{github.Source: poster{g}}
	return answer.New(answer.Deps{Tasks: tasks, Reads: db, Deliveries: db, Posters: posters, Clock: systemClock{}})
}

// poster posts as the App the settings hold when it posts.
type poster struct{ g *githubApp }

func (poster) Target(ev domain.Event) (domain.DeliveryTarget, bool) {
	return (&github.Adapter{}).Target(ev)
}

func (poster) Body(d domain.Delivery) string { return (&github.Adapter{}).Body(d) }

func (p poster) Find(ctx context.Context, t domain.DeliveryTarget) (string, bool, error) {
	a, err := p.g.adapter(ctx)
	if err != nil {
		return "", false, err
	}
	return a.Find(ctx, t) //nolint:wrapcheck // the answer service records the adapter's own error
}

func (p poster) Post(ctx context.Context, t domain.DeliveryTarget, body string) (string, error) {
	a, err := p.g.adapter(ctx)
	if err != nil {
		return "", err
	}
	return a.Post(ctx, t, body) //nolint:wrapcheck // the answer service records the adapter's own error
}

func (p poster) Edit(ctx context.Context, t domain.DeliveryTarget, externalID, body string) error {
	a, err := p.g.adapter(ctx)
	if err != nil {
		return err
	}
	return a.Edit(ctx, t, externalID, body) //nolint:wrapcheck // the answer service records the adapter's own error
}

// links reads the pull request links that Run is given, as the App the settings hold.
type links struct{ g *githubApp }

func (l links) Occurrence(ctx context.Context, link string) (domain.Occurrence, error) {
	a, err := l.g.adapter(ctx)
	if errors.Is(err, errNoApp) {
		return domain.Occurrence{}, fmt.Errorf("%w: the portal has no GitHub App in its settings", domain.ErrRunUnsupported)
	}
	if err != nil {
		return domain.Occurrence{}, err
	}
	occ, err := a.Occurrence(ctx, link)
	if errors.Is(err, github.ErrNotPullRequestLink) {
		return occ, fmt.Errorf("%w: %w", domain.ErrBadLink, err)
	}
	if err != nil {
		return occ, fmt.Errorf("github: %w", err)
	}
	return occ, nil
}

// txStore lends the store's transactions to the inbound service.
type txStore struct{ db *store.Store }

func (s txStore) InTx(ctx context.Context, fn func(inbound.Tx) error) error {
	if err := s.db.InTx(ctx, func(tx *store.Store) error { return fn(tx) }); err != nil {
		return fmt.Errorf("transaction: %w", err)
	}
	return nil
}

// tokenTimeout bounds a mint, so a slow GitHub cannot hold a proxy's request for long.
const tokenTimeout = 30 * time.Second
