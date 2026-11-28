package main

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/githome"
	"github.com/IniZio/moor/internal/adapter/githome/githometest"
	"github.com/IniZio/moor/internal/domain"
)

func silentLog() *slog.Logger { return slog.New(slog.NewTextHandler(io.Discard, nil)) }

const (
	homePath  = "example-org/team-home"
	homeToken = "read-token"
)

var homeRepoName = domain.Repo{Host: "github.com", Path: homePath}

// homeRig is a team home on a git server and the replicas that fetch it.
type homeRig struct {
	t      *testing.T
	git    *githometest.Server
	mu     sync.Mutex
	tokens []domain.Repo
	noApp  bool
}

func newHomeRig(t *testing.T) *homeRig {
	t.Helper()
	return &homeRig{t: t, git: githometest.New(t, homePath, homeToken)}
}

func (r *homeRig) commit(branch string, files map[string]string) string {
	r.t.Helper()
	hash, err := r.git.Commit(branch, files)
	if err != nil {
		r.t.Fatal(err)
	}
	return hash
}

func (r *homeRig) token(_ context.Context, repo domain.Repo) (string, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.tokens = append(r.tokens, repo)
	if r.noApp {
		return "", errNoApp
	}
	return homeToken, nil
}

// replica starts a replica that fetches the home on branch and again every interval.
func (r *homeRig) replica(branch string, interval time.Duration) *homeRepo {
	r.t.Helper()
	h := &homeRepo{repo: homeRepoName, branch: branch, gitBase: r.git.URL(), token: r.token, fetch: githome.Fetch, log: silentLog()}
	h.start(context.Background(), interval)
	r.t.Cleanup(h.close)
	return h
}

func specNames(t *testing.T, h *homeRepo) (names []string, problem string) {
	t.Helper()
	files, err := h.TaskSpecs(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range files {
		if f.File == homeNotice {
			problem = f.Problem
			continue
		}
		names = append(names, string(f.Spec.Name))
	}
	return names, problem
}

func hasSpecs(t *testing.T, h *homeRepo, want ...string) func() bool {
	return func() bool {
		got, _ := specNames(t, h)
		return strings.Join(got, ",") == strings.Join(want, ",")
	}
}

func idle(h *homeRepo) func() bool {
	return func() bool {
		h.mu.Lock()
		defer h.mu.Unlock()
		return !h.working
	}
}

func spec(name string) string {
	return "name: " + name + "\non:\n  workflow_dispatch:\nprompt: p\n"
}

func TestTheHomeIsFetchedAtStartWithItsCommitOnEverySpec(t *testing.T) {
	r := newHomeRig(t)
	want := r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha"), "tasks/b.yaml": spec("beta")})
	h := r.replica("", time.Hour)
	eventually(t, "the first fetch", hasSpecs(t, h, "alpha", "beta"))

	files, _ := h.TaskSpecs(context.Background())
	for _, f := range files {
		if f.Commit != domain.Commit(want) {
			t.Errorf("%s at %q, want %s", f.File, f.Commit, want)
		}
	}
	if _, problem := specNames(t, h); problem != "" {
		t.Errorf("a good fetch shows %q", problem)
	}
	if len(r.tokens) == 0 || r.tokens[0] != homeRepoName {
		t.Errorf("tokens were asked for %v, want only %v", r.tokens, homeRepoName)
	}
}

func TestAPushToTheTrackedBranchShowsTheNewSpecWithoutARestart(t *testing.T) {
	r := newHomeRig(t)
	r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha")})
	h := r.replica("", time.Hour)
	eventually(t, "the first fetch", hasSpecs(t, h, "alpha"))

	next := r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha"), "tasks/b.yaml": spec("beta")})
	h.Pushed(homeRepoName, "main")
	eventually(t, "the pushed spec", hasSpecs(t, h, "alpha", "beta"))
	files, _ := h.TaskSpecs(context.Background())
	if files[0].Commit != domain.Commit(next) {
		t.Errorf("commit %s, want %s", files[0].Commit, next)
	}
}

func TestAPushToAnotherBranchOrRepositoryIsIgnored(t *testing.T) {
	r := newHomeRig(t)
	r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha")})
	h := r.replica("", time.Hour)
	eventually(t, "the first fetch", hasSpecs(t, h, "alpha"))
	eventually(t, "the worker to rest", idle(h))
	before := len(r.git.Requests())

	r.commit("feature", map[string]string{"tasks/a.yaml": spec("alpha"), "tasks/b.yaml": spec("beta")})
	h.Pushed(homeRepoName, "feature")
	h.Pushed(domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, "main")
	time.Sleep(100 * time.Millisecond)
	eventually(t, "the worker to rest", idle(h))
	if got := len(r.git.Requests()); got != before {
		t.Fatalf("%d requests after an ignored push, %d before", got, before)
	}
	if names, _ := specNames(t, h); len(names) != 1 {
		t.Fatalf("specs %v", names)
	}
}

func TestANamedBranchIsTheOneFollowed(t *testing.T) {
	r := newHomeRig(t)
	r.commit("main", map[string]string{"tasks/a.yaml": spec("on-main")})
	r.commit("release", map[string]string{"tasks/a.yaml": spec("on-release")})
	h := r.replica("release", time.Hour)
	eventually(t, "the first fetch", hasSpecs(t, h, "on-release"))

	r.commit("release", map[string]string{"tasks/a.yaml": spec("on-release-2")})
	h.Pushed(homeRepoName, "main")
	h.Pushed(homeRepoName, "release")
	eventually(t, "the release push", hasSpecs(t, h, "on-release-2"))
}

func TestAFailedFetchKeepsTheLastGoodCopyAndSaysWhy(t *testing.T) {
	r := newHomeRig(t)
	good := r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha")})
	h := r.replica("", time.Hour)
	eventually(t, "the first fetch", hasSpecs(t, h, "alpha"))
	eventually(t, "the worker to rest", idle(h))

	r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha"), "tasks/b.yaml": spec("beta")})
	r.git.Fail(1)
	h.Pushed(homeRepoName, "main")
	eventually(t, "the reason", func() bool { _, p := specNames(t, h); return p != "" })

	names, problem := specNames(t, h)
	if strings.Join(names, ",") != "alpha" {
		t.Errorf("specs %v after a failed fetch, want the last good copy", names)
	}
	for _, want := range []string{homePath, "500", domain.Commit(good).Short(), "could not be fetched again"} {
		if !strings.Contains(problem, want) {
			t.Errorf("the notice %q lacks %q", problem, want)
		}
	}
	if strings.Contains(problem, homeToken) {
		t.Errorf("the notice names the token: %q", problem)
	}

	h.Pushed(homeRepoName, "main")
	eventually(t, "the next fetch", hasSpecs(t, h, "alpha", "beta"))
	if _, problem := specNames(t, h); problem != "" {
		t.Errorf("a good fetch still shows %q", problem)
	}
}

func TestWithoutAnAppThereAreNoSpecsAndTheReasonIsShown(t *testing.T) {
	r := newHomeRig(t)
	r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha")})
	r.noApp = true
	h := r.replica("", 20*time.Millisecond)
	eventually(t, "the reason", func() bool { _, p := specNames(t, h); return strings.Contains(p, errNoApp.Error()) })
	if names, problem := specNames(t, h); len(names) != 0 || !strings.Contains(problem, "There are no task specs") {
		t.Fatalf("specs %v, notice %q", names, problem)
	}
	if len(r.git.Requests()) != 0 {
		t.Fatal("the git server was asked without a token")
	}

	r.mu.Lock()
	r.noApp = false
	r.mu.Unlock()
	eventually(t, "the timer to fetch once an App exists", hasSpecs(t, h, "alpha"))
}

func TestTwoReplicasEachConvergeOnTheNewestCommit(t *testing.T) {
	r := newHomeRig(t)
	r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha")})
	first, second := r.replica("", 30*time.Millisecond), r.replica("", 30*time.Millisecond)
	eventually(t, "both to fetch", func() bool { return hasSpecs(t, first, "alpha")() && hasSpecs(t, second, "alpha")() })

	r.commit("main", map[string]string{"tasks/a.yaml": spec("alpha"), "tasks/b.yaml": spec("beta")})
	first.Pushed(homeRepoName, "main")
	eventually(t, "the replica told of the push", hasSpecs(t, first, "alpha", "beta"))
	eventually(t, "the replica the push missed, by its timer", hasSpecs(t, second, "alpha", "beta"))
}

func TestAFailureThatNamesTheTokenIsRedacted(t *testing.T) {
	h := &homeRepo{
		repo: homeRepoName, token: func(context.Context, domain.Repo) (string, error) { return "secret-token", nil },
		fetch: func(context.Context, githome.Remote) (githome.Snapshot, error) {
			return githome.Snapshot{}, errors.New("rejected secret-token")
		},
		log: silentLog(),
	}
	if err := h.refresh(context.Background()); err == nil || strings.Contains(err.Error(), "secret-token") {
		t.Fatalf("error %v", err)
	}
}
