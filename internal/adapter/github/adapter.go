package github

import (
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/bradleyfalzon/ghinstallation/v2"
	"github.com/google/go-github/v88/github"
)

// Source is the event source this adapter serves.
const Source = "github"

const apiTimeout = 30 * time.Second

// Config is the GitHub App the portal acts as, as the settings' github_app credential holds it. Key is the PEM
// of the App's private key. Without WebhookSecret the hook refuses every delivery.
type Config struct {
	AppID         int64
	Key           []byte
	WebhookSecret []byte
	APIBase       string
}

// Check says which part is missing.
func (c Config) Check() error {
	var missing []string
	for name, ok := range map[string]bool{
		"app id": c.AppID != 0, "private key": len(c.Key) > 0, "API base URL": c.APIBase != "",
	} {
		if !ok {
			missing = append(missing, name)
		}
	}
	if len(missing) > 0 {
		return fmt.Errorf("the GitHub App needs its %s", strings.Join(missing, ", "))
	}
	return nil
}

// Adapter talks to GitHub as the App. It holds one client for the App itself and one for each installation it has
// been asked about, since a client keeps its installation's token until it expires.
type Adapter struct {
	secret  []byte
	base    string
	apps    *github.Client
	atr     *ghinstallation.AppsTransport
	clients sync.Map
}

// New reads the App's private key and builds the adapter. It makes no request.
func New(cfg Config) (*Adapter, error) {
	if err := cfg.Check(); err != nil {
		return nil, err
	}
	atr, err := ghinstallation.NewAppsTransport(http.DefaultTransport, cfg.AppID, cfg.Key)
	if err != nil {
		return nil, fmt.Errorf("github app key: %w", err)
	}
	base := strings.TrimRight(cfg.APIBase, "/")
	atr.BaseURL = base
	a := &Adapter{secret: cfg.WebhookSecret, base: base + "/", atr: atr}
	if a.apps, err = a.newClient(atr); err != nil {
		return nil, err
	}
	return a, nil
}

func (a *Adapter) newClient(tr http.RoundTripper) (*github.Client, error) {
	c, err := github.NewClient(github.WithHTTPClient(&http.Client{Transport: tr, Timeout: apiTimeout}), github.WithURLs(&a.base, &a.base))
	if err != nil {
		return nil, fmt.Errorf("github client: %w", err)
	}
	return c, nil
}
