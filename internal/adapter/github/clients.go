package github

import (
	"context"
	"errors"
	"fmt"
	"net/http"

	"github.com/bradleyfalzon/ghinstallation/v2"
	"github.com/google/go-github/v88/github"
)

var errNoInstallation = errors.New("the GitHub App is not installed on the repository")

// forRepo gives the client that acts as the App's installation on the repository.
func (a *Adapter) forRepo(ctx context.Context, owner, repo string) (*github.Client, error) {
	inst, resp, err := a.apps.Apps.GetRepositoryInstallation(ctx, owner, repo)
	if err != nil {
		if resp != nil && resp.StatusCode == http.StatusNotFound {
			return nil, fmt.Errorf("%s/%s: %w", owner, repo, errNoInstallation)
		}
		return nil, fmt.Errorf("find the installation on %s/%s: %w", owner, repo, err)
	}
	if c, ok := a.clients.Load(inst.GetID()); ok {
		if client, ok := c.(*github.Client); ok {
			return client, nil
		}
	}
	c, err := a.newClient(ghinstallation.NewFromAppsTransport(a.atr, inst.GetID()))
	if err != nil {
		return nil, err
	}
	a.clients.Store(inst.GetID(), c)
	return c, nil
}
