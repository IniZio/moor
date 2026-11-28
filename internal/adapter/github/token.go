package github

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/bradleyfalzon/ghinstallation/v2"
	"github.com/google/go-github/v88/github"
)

// taskPermissions are what a task's token may do: push branches, open and edit pull requests, and comment. It
// cannot administer the repository.
var taskPermissions = github.InstallationPermissions{
	Contents: github.Ptr("write"), PullRequests: github.Ptr("write"), Issues: github.Ptr("write"),
}

// readPermissions are what a token that only reads a repository may do.
var readPermissions = github.InstallationPermissions{Contents: github.Ptr("read")}

// TaskToken mints an installation token limited to the one repository, owner/name, with taskPermissions.
func (a *Adapter) TaskToken(ctx context.Context, repo string) (string, time.Time, error) {
	return a.token(ctx, repo, taskPermissions)
}

// ReadToken mints an installation token limited to the one repository, owner/name, that can only read its contents.
func (a *Adapter) ReadToken(ctx context.Context, repo string) (string, time.Time, error) {
	return a.token(ctx, repo, readPermissions)
}

func (a *Adapter) token(ctx context.Context, repo string, perms github.InstallationPermissions) (string, time.Time, error) {
	owner, name, ok := strings.Cut(repo, "/")
	if !ok || owner == "" || name == "" || strings.Contains(name, "/") {
		return "", time.Time{}, fmt.Errorf("%q is not owner/name", repo)
	}
	inst, resp, err := a.apps.Apps.GetRepositoryInstallation(ctx, owner, name)
	if err != nil {
		if resp != nil && resp.StatusCode == 404 {
			return "", time.Time{}, fmt.Errorf("%s: %w", repo, errNoInstallation)
		}
		return "", time.Time{}, fmt.Errorf("find the installation on %s: %w", repo, err)
	}
	tr := ghinstallation.NewFromAppsTransport(a.atr, inst.GetID())
	tr.InstallationTokenOptions = &github.InstallationTokenOptions{Repositories: []string{name}, Permissions: &perms}
	token, err := tr.Token(ctx)
	if err != nil {
		return "", time.Time{}, fmt.Errorf("mint a token for %s: %w", repo, err)
	}
	expires, _, err := tr.Expiry()
	if err != nil {
		return "", time.Time{}, fmt.Errorf("expiry of the token for %s: %w", repo, err)
	}
	return token, expires, nil
}
