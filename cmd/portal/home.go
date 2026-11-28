package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/IniZio/moor/internal/adapter/githome"
	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
)

// homeNotice is the file the screens list a problem with the home under, since the problem belongs to no spec file.
const homeNotice = "team home"

// fetchTimeout bounds one fetch of the home, so a stalled server cannot hold the replica's one fetch for long.
const fetchTimeout = time.Minute

// homeRepo is the team's home fetched from its forge. Each replica keeps its own copy. A fetch that fails leaves
// the last good copy in place and shows why on the Tasks screen, beside the problems of the spec files.
type homeRepo struct {
	repo    domain.Repo
	branch  string
	gitBase string
	token   func(context.Context, domain.Repo) (string, error)
	fetch   func(context.Context, githome.Remote) (githome.Snapshot, error)
	log     *slog.Logger

	mu      sync.Mutex
	snap    *githome.Snapshot
	failure string
	closed  bool
	again   bool
	working bool
	ctx     context.Context
	stop    context.CancelFunc
	tasks   sync.WaitGroup
}

func newHomeRepo(cfg homeConfig, hub *githubApp) *homeRepo {
	return &homeRepo{
		repo: cfg.repo, branch: cfg.branch, gitBase: cfg.gitBase, token: hub.readToken, fetch: githome.Fetch, log: slog.Default(),
	}
}

// readToken mints a token of the settings' App that reads the one repository.
func (g *githubApp) readToken(ctx context.Context, repo domain.Repo) (string, error) {
	a, err := g.adapter(ctx)
	if err != nil {
		return "", err
	}
	ctx, cancel := context.WithTimeout(ctx, tokenTimeout)
	defer cancel()
	token, _, err := a.ReadToken(ctx, string(repo.Path))
	if err != nil {
		return "", fmt.Errorf("github: %w", err)
	}
	return token, nil
}

func (h *homeRepo) url() string {
	base := h.gitBase
	if base == "" {
		base = "https://" + string(h.repo.Host)
	}
	return base + "/" + string(h.repo.Path) + ".git"
}

// start fetches the home at once and then every interval, until close. Both happen in the background, so the
// portal serves while the first fetch runs.
func (h *homeRepo) start(parent context.Context, interval time.Duration) {
	h.mu.Lock()
	h.ctx, h.stop = context.WithCancel(parent)
	h.mu.Unlock()
	h.refreshSoon()
	h.tasks.Add(1)
	go func() {
		defer h.tasks.Done()
		tick := time.NewTicker(interval)
		defer tick.Stop()
		for {
			select {
			case <-h.ctx.Done():
				return
			case <-tick.C:
				h.refreshSoon()
			}
		}
	}()
}

// close stops fetching and waits for a fetch under way.
func (h *homeRepo) close() {
	h.mu.Lock()
	h.closed = true
	stop := h.stop
	h.mu.Unlock()
	if stop != nil {
		stop()
	}
	h.tasks.Wait()
}

// refreshSoon asks for a fetch. Asks that come while one runs make one more after it, which reads the newest push.
func (h *homeRepo) refreshSoon() {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.again = true
	if h.working || h.closed {
		return
	}
	h.working = true
	h.tasks.Add(1)
	go func() {
		defer h.tasks.Done()
		for h.next() {
			_ = h.refresh(h.ctx)
		}
	}()
}

// next says whether a fetch is asked for, and ends the worker when none is.
func (h *homeRepo) next() bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	if !h.again || h.closed {
		h.working = false
		return false
	}
	h.again = false
	return true
}

// refresh fetches the home once. A failure keeps the last good copy and is the notice the screens show.
func (h *homeRepo) refresh(ctx context.Context) error {
	ctx, cancel := context.WithTimeout(ctx, fetchTimeout)
	defer cancel()
	snap, err := h.pull(ctx)
	h.mu.Lock()
	defer h.mu.Unlock()
	if err != nil {
		h.failure = err.Error()
		h.log.Warn("team home not fetched", "repo", h.repo, "err", err)
		return err
	}
	if h.snap == nil || h.snap.Commit != snap.Commit {
		h.log.Info("team home fetched", "repo", h.repo, "branch", snap.Branch, "commit", domain.Commit(snap.Commit).Short())
	}
	h.snap, h.failure = &snap, ""
	return nil
}

func (h *homeRepo) pull(ctx context.Context) (githome.Snapshot, error) {
	token, err := h.token(ctx, h.repo)
	if err != nil {
		return githome.Snapshot{}, err
	}
	snap, err := h.fetch(ctx, githome.Remote{URL: h.url(), Branch: h.branch, Token: token})
	if err != nil {
		return githome.Snapshot{}, errors.New(strings.ReplaceAll(err.Error(), token, "***"))
	}
	return snap, nil
}

// Pushed refreshes the home when the push is to the branch it follows, which is the named branch, or else the
// branch of the copy it holds, or any branch while it holds none.
func (h *homeRepo) Pushed(repo domain.Repo, branch string) {
	if repo != h.repo || (h.followed() != "" && branch != h.followed()) {
		return
	}
	h.log.Info("push to the team home", "repo", repo, "branch", branch)
	h.refreshSoon()
}

func (h *homeRepo) followed() string {
	h.mu.Lock()
	defer h.mu.Unlock()
	switch {
	case h.branch != "":
		return h.branch
	case h.snap != nil:
		return h.snap.Branch
	}
	return ""
}

// TaskSpecs lists the specs of the copy the replica holds, each with its commit, and one more entry that says why
// the copy is missing or old when the last fetch failed or none has finished.
func (h *homeRepo) TaskSpecs(ctx context.Context) ([]domain.SpecFile, error) {
	h.mu.Lock()
	snap, failure := h.snap, h.failure
	h.mu.Unlock()
	var out []domain.SpecFile
	if snap != nil {
		files, err := yamlfile.NewHome(snap.Files).TaskSpecs(ctx)
		if err != nil {
			return nil, fmt.Errorf("read the team home: %w", err)
		}
		for i := range files {
			files[i].Commit = domain.Commit(snap.Commit)
		}
		out = files
	}
	if problem := h.problem(snap, failure); problem != "" {
		out = append(out, domain.SpecFile{File: homeNotice, Problem: problem})
	}
	return out, nil
}

func (h *homeRepo) problem(snap *githome.Snapshot, failure string) string {
	switch {
	case failure != "" && snap != nil:
		return fmt.Sprintf("The team home %s could not be fetched again: %s. The task specs are those of commit %s.", h.repo, failure, domain.Commit(snap.Commit).Short())
	case failure != "":
		return fmt.Sprintf("The team home %s could not be fetched: %s. There are no task specs.", h.repo, failure)
	case snap == nil:
		return fmt.Sprintf("The team home %s is being fetched. There are no task specs yet.", h.repo)
	}
	return ""
}
