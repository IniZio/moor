package main

import (
	"encoding/base64"
	"errors"
	"flag"
	"fmt"
	"io"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/adapter/secretbox"
)

type config struct {
	listen         string
	database       string
	userHeader     string
	orgName        string
	homeDir        string
	publicURL      string
	linkTick       time.Duration
	trustedOrigins []string
	secretKey      []byte
}

const usage = `portal [flags]

Each flag has an environment variable. PORTAL_SECRET_KEY has no flag: it is the base64 of a 32-byte key.`

// parseConfig reads flags, each defaulting to its environment variable. The user header has no default,
// because which header the sign-in proxy sets is the operator's to say.
func parseConfig(args []string, getenv func(string) string) (config, error) {
	var c config
	var origins string
	fs := flag.NewFlagSet("portal", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	fs.StringVar(&c.listen, "listen", orDefault(getenv("PORTAL_LISTEN"), "127.0.0.1:8080"), "address to listen on (PORTAL_LISTEN)")
	fs.StringVar(&c.database, "database-url", getenv("PORTAL_DATABASE_URL"), "Postgres URL (PORTAL_DATABASE_URL)")
	fs.StringVar(&c.userHeader, "user-header", getenv("PORTAL_USER_HEADER"), "request header that carries the signed-in person (PORTAL_USER_HEADER)")
	fs.StringVar(&c.homeDir, "home-dir", getenv("PORTAL_HOME_DIR"), "folder holding the checked-out team home, whose tasks/*.yaml are the task specs (PORTAL_HOME_DIR); without it there are no task specs")
	fs.StringVar(&c.publicURL, "public-url", getenv("PORTAL_PUBLIC_URL"), "address runners reach the portal at, such as https://portal.example.com (PORTAL_PUBLIC_URL); without it no machine can join or link")
	fs.StringVar(&c.orgName, "org-name", getenv("PORTAL_ORG_NAME"), "organisation name shown on Settings (PORTAL_ORG_NAME)")
	fs.StringVar(&origins, "trusted-origins", getenv("PORTAL_TRUSTED_ORIGINS"), "comma-separated origins allowed to post when the proxy rewrites the host (PORTAL_TRUSTED_ORIGINS)")
	if err := fs.Parse(args); err != nil {
		return config{}, fmt.Errorf("%w\n%s", err, usage)
	}
	for _, o := range strings.Split(origins, ",") {
		if o = strings.TrimSpace(o); o != "" {
			c.trustedOrigins = append(c.trustedOrigins, o)
		}
	}
	key, err := base64.StdEncoding.DecodeString(getenv("PORTAL_SECRET_KEY"))
	if err != nil || len(key) != secretbox.KeySize {
		return config{}, errors.New("PORTAL_SECRET_KEY must be the base64 of a 32-byte key")
	}
	c.secretKey = key
	for name, val := range map[string]string{"database-url": c.database, "user-header": c.userHeader} {
		if val == "" {
			return config{}, fmt.Errorf("%s is required, as a flag or its environment variable", name)
		}
	}
	return c, nil
}

func orDefault(v, fallback string) string {
	if v == "" {
		return fallback
	}
	return v
}
