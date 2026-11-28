package main

import (
	"encoding/base64"
	"errors"
	"flag"
	"fmt"
	"io"
	"strconv"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/secretbox"
)

type config struct {
	listen         string
	database       string
	userHeader     string
	orgName        string
	homeDir        string
	publicURL      string
	internalURL    string
	internalSecret string
	linkTick       time.Duration
	// keepWindow is how long a done or failed task keeps its containers, so a person can reopen or look.
	keepWindow time.Duration
	// runnerArgs is what the join command carries after its own flags, such as the image flags of the runner.
	runnerArgs     string
	trustedOrigins []string
	secretKey      []byte
	// github is the GitHub App. With none of it set the portal serves no /hooks/github.
	github github.Config
}

const usage = `portal [flags]

Each flag has an environment variable. PORTAL_SECRET_KEY has no flag: it is the base64 of a 32-byte key.
PORTAL_INTERNAL_SECRET has no flag either: every replica gets the same one, and with -internal-url it
lets a terminal hop to the replica that holds a machine's link.

PORTAL_GITHUB_WEBHOOK_SECRET has no flag: it is the secret that signs GitHub's deliveries. It and the three
github flags go together, none has a default, and with none of them the portal serves no /hooks/github.`

// parseConfig reads flags, each defaulting to its environment variable. The user header has no default,
// because which header the sign-in proxy sets is the operator's to say.
func parseConfig(args []string, getenv func(string) string) (config, error) {
	var c config
	var origins, appID, keep string
	var err error
	fs := flag.NewFlagSet("portal", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	fs.StringVar(&c.listen, "listen", orDefault(getenv("PORTAL_LISTEN"), "127.0.0.1:8080"), "address to listen on (PORTAL_LISTEN)")
	fs.StringVar(&c.database, "database-url", getenv("PORTAL_DATABASE_URL"), "Postgres URL (PORTAL_DATABASE_URL)")
	fs.StringVar(&c.userHeader, "user-header", getenv("PORTAL_USER_HEADER"), "request header that carries the signed-in person (PORTAL_USER_HEADER)")
	fs.StringVar(&c.homeDir, "home-dir", getenv("PORTAL_HOME_DIR"), "folder holding the checked-out team home, whose tasks/*.yaml are the task specs (PORTAL_HOME_DIR); without it there are no task specs")
	fs.StringVar(&c.publicURL, "public-url", getenv("PORTAL_PUBLIC_URL"), "address runners reach the portal at, such as https://portal.example.com (PORTAL_PUBLIC_URL); without it no machine can join or link")
	fs.StringVar(&c.internalURL, "internal-url", getenv("PORTAL_INTERNAL_URL"), "address other portal replicas reach this one at, such as http://10.0.0.5:8080 (PORTAL_INTERNAL_URL); without it no terminal hops here")
	fs.StringVar(&c.orgName, "org-name", getenv("PORTAL_ORG_NAME"), "organisation name shown on Settings (PORTAL_ORG_NAME)")
	fs.StringVar(&origins, "trusted-origins", getenv("PORTAL_TRUSTED_ORIGINS"), "comma-separated origins allowed to post when the proxy rewrites the host (PORTAL_TRUSTED_ORIGINS)")
	fs.StringVar(&appID, "github-app-id", getenv("PORTAL_GITHUB_APP_ID"), "id of the GitHub App that answers pull requests (PORTAL_GITHUB_APP_ID)")
	fs.StringVar(&c.github.KeyFile, "github-private-key-file", getenv("PORTAL_GITHUB_PRIVATE_KEY_FILE"), "file holding the GitHub App's private key (PORTAL_GITHUB_PRIVATE_KEY_FILE)")
	fs.StringVar(&c.github.APIBase, "github-api-url", getenv("PORTAL_GITHUB_API_URL"), "address of GitHub's API, such as https://api.github.com/ (PORTAL_GITHUB_API_URL)")
	fs.StringVar(&c.runnerArgs, "runner-args", getenv("PORTAL_RUNNER_ARGS"), "flags the join command ends with, such as the runner's image flags (PORTAL_RUNNER_ARGS)")
	fs.StringVar(&keep, "keep-window", orDefault(getenv("PORTAL_KEEP_WINDOW"), defaultKeepWindow), "how long a done or failed task keeps its containers, such as 24h (PORTAL_KEEP_WINDOW)")
	if err := fs.Parse(args); err != nil {
		return config{}, fmt.Errorf("%w\n%s", err, usage)
	}
	if err = c.readOperations(keep); err != nil {
		return config{}, err
	}
	c.trustedOrigins = splitList(origins)
	key, err := base64.StdEncoding.DecodeString(getenv("PORTAL_SECRET_KEY"))
	if err != nil || len(key) != secretbox.KeySize {
		return config{}, errors.New("PORTAL_SECRET_KEY must be the base64 of a 32-byte key")
	}
	c.secretKey = key
	c.internalSecret = getenv("PORTAL_INTERNAL_SECRET")
	if err := readGitHub(&c.github, appID, getenv("PORTAL_GITHUB_WEBHOOK_SECRET")); err != nil {
		return config{}, err
	}
	for name, val := range map[string]string{"database-url": c.database, "user-header": c.userHeader} {
		if val == "" {
			return config{}, fmt.Errorf("%s is required, as a flag or its environment variable", name)
		}
	}
	return c, nil
}

// splitList reads a comma-separated list, dropping blanks.
func splitList(text string) []string {
	var out []string
	for _, o := range strings.Split(text, ",") {
		if o = strings.TrimSpace(o); o != "" {
			out = append(out, o)
		}
	}
	return out
}

// readGitHub sets the App's id from its text and its webhook secret, and checks that the App is whole or not given at all.
func readGitHub(c *github.Config, appID, webhookSecret string) error {
	c.WebhookSecret = []byte(webhookSecret)
	if appID != "" {
		n, err := strconv.ParseInt(strings.TrimSpace(appID), 10, 64)
		if err != nil || n <= 0 {
			return errors.New("github-app-id must be the number of the GitHub App")
		}
		c.AppID = n
	}
	if !c.Enabled() {
		return nil
	}
	if err := c.Check(); err != nil {
		return fmt.Errorf("github app: %w", err)
	}
	return nil
}

// defaultKeepWindow is a day: long enough to reopen a task the next morning, short enough that a host
// does not fill with the containers of finished tasks. The spec leaves the length open.
const defaultKeepWindow = "24h"

// readOperations reads what the operator sets about running tasks and machines: the keep window and the
// runner args, which must stay one line because a person pastes the join command into a shell.
func (c *config) readOperations(keep string) error {
	d, err := time.ParseDuration(keep)
	if err != nil || d <= 0 {
		return errors.New("keep-window must be a duration above zero, such as 24h")
	}
	c.keepWindow = d
	if strings.ContainsAny(c.runnerArgs, "\r\n") {
		return errors.New("runner-args must be on one line")
	}
	return nil
}

func orDefault(v, fallback string) string {
	if v == "" {
		return fallback
	}
	return v
}
