package main

import (
	"errors"
	"flag"
	"fmt"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// defaultHomeRefresh is how often a replica fetches the home again when no push told it to. It is the safety
// net for a push whose webhook never came, and for a replica the webhook did not reach.
const defaultHomeRefresh = "5m"

// homeConfig says where the portal fetches the team's home from. It is the operator's to set, beside homeDir.
type homeConfig struct {
	text    string
	refresh string
	repo    domain.Repo
	// branch is empty for the repository's default branch.
	branch string
	// gitBase replaces https://<host> as the address of the git server, which only tests change.
	gitBase string
	every   time.Duration
}

func (h *homeConfig) bind(fs *flag.FlagSet, getenv func(string) string) {
	fs.StringVar(&h.text, "home-repo", getenv("PORTAL_HOME_REPO"), "repository the portal fetches the team home from, as github.com/owner/name or github.com/owner/name@branch (PORTAL_HOME_REPO); the default branch without @branch; not with -home-dir")
	fs.StringVar(&h.refresh, "home-refresh", orDefault(getenv("PORTAL_HOME_REFRESH"), defaultHomeRefresh), "how often each replica fetches the home again besides on a push, such as 5m (PORTAL_HOME_REFRESH)")
}

// checkHome reads what bind set, and refuses a home that is both a folder and a repository.
func (c *config) checkHome() error {
	every, err := time.ParseDuration(c.home.refresh)
	if err != nil || every <= 0 {
		return errors.New("home-refresh must be a duration above zero, such as 5m")
	}
	c.home.every = every
	if c.home.text == "" {
		return nil
	}
	if c.homeDir != "" {
		return errors.New("home-repo and home-dir are exclusive: the home is a repository to fetch or a folder that is already there")
	}
	text, branch, named := strings.Cut(c.home.text, "@")
	repo, err := domain.ParseRepo(text)
	switch {
	case err != nil:
		return fmt.Errorf("home-repo: %w", err)
	case repo.Host != domain.DefaultRepoHost || strings.Count(string(repo.Path), "/") != 1:
		return fmt.Errorf("home-repo: %q is not github.com/owner/name, the only forge a GitHub App reaches", text)
	case named && branch == "":
		return errors.New("home-repo: a name after @ is needed, or no @")
	}
	c.home.repo, c.home.branch = repo, branch
	return nil
}
