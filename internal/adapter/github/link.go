package github

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"strconv"
	"strings"

	"github.com/google/go-github/v88/github"

	"github.com/IniZio/moor/internal/domain"
)

// ErrNotPullRequestLink is a Run whose link is not a pull request on github.com.
var ErrNotPullRequestLink = errors.New("not a pull request link")

// parseLink reads https://github.com/owner/name/pull/91.
func parseLink(link string) (owner, name string, number int, err error) {
	u, perr := url.Parse(strings.TrimSpace(link))
	if perr != nil || (u.Scheme != "https" && u.Scheme != "http") || !strings.EqualFold(u.Host, string(domain.DefaultRepoHost)) {
		return "", "", 0, ErrNotPullRequestLink
	}
	parts := strings.Split(strings.Trim(u.Path, "/"), "/")
	if len(parts) != 4 || parts[2] != "pull" {
		return "", "", 0, ErrNotPullRequestLink
	}
	n, nerr := strconv.Atoi(parts[3])
	if nerr != nil || n <= 0 {
		return "", "", 0, ErrNotPullRequestLink
	}
	return parts[0], parts[1], n, nil
}

// Occurrence fetches the pull request a link names and reads it into the same event that its webhook makes,
// except that it has no action, since nobody opened or updated it just now.
func (a *Adapter) Occurrence(ctx context.Context, link string) (domain.Occurrence, error) {
	owner, name, number, err := parseLink(link)
	if err != nil {
		return domain.Occurrence{}, err
	}
	c, err := a.forRepo(ctx, owner, name)
	if err != nil {
		return domain.Occurrence{}, err
	}
	pr, _, err := c.PullRequests.Get(ctx, owner, name, number)
	if err != nil {
		return domain.Occurrence{}, fmt.Errorf("fetch pull request %s/%s#%d: %w", owner, name, number, err)
	}
	repo := pr.GetBase().GetRepo()
	if repo == nil {
		repo = &github.Repository{FullName: github.Ptr(owner + "/" + name)}
	}
	return prEventOf("", repo, pr, "").occurrence(domain.EventWorkflowDispatch, "", pr.GetUser().GetLogin())
}
