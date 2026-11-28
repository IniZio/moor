package github

import (
	"context"
	"fmt"
	"strings"
)

// DefaultBranch reads the repository's default branch, owner/name, as the App's installation on it.
func (a *Adapter) DefaultBranch(ctx context.Context, repo string) (string, error) {
	owner, name, ok := strings.Cut(repo, "/")
	if !ok || owner == "" || name == "" || strings.Contains(name, "/") {
		return "", fmt.Errorf("%q is not owner/name", repo)
	}
	c, err := a.forRepo(ctx, owner, name)
	if err != nil {
		return "", err
	}
	r, _, err := c.Repositories.Get(ctx, owner, name)
	if err != nil {
		return "", fmt.Errorf("read %s: %w", repo, err)
	}
	if r.GetDefaultBranch() == "" {
		return "", fmt.Errorf("%s names no default branch", repo)
	}
	return r.GetDefaultBranch(), nil
}
