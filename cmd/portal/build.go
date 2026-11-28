package main

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/adapter/secretbox"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/settings"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/web"
)

type systemClock struct{}

func (systemClock) Now() time.Time { return time.Now().UTC() }

// build opens the store and wires the settings service into the web handler. cmd is the one place that
// knows every adapter.
func build(ctx context.Context, cfg config) (http.Handler, func(), error) {
	db, err := store.Open(ctx, cfg.database)
	if err != nil {
		return nil, nil, fmt.Errorf("open the database: %w", err)
	}
	box, err := secretbox.New(cfg.secretKey)
	if err != nil {
		db.Close()
		return nil, nil, fmt.Errorf("secret key: %w", err)
	}
	svc := settings.New(db, yamlParser{}, box, systemClock{})
	handler, err := web.New(
		web.Config{UserHeader: cfg.userHeader, OrgName: cfg.orgName, TrustedOrigins: cfg.trustedOrigins},
		web.Deps{Settings: svc, Specs: specSource(cfg.homeDir), Start: task.New(db, systemClock{}, task.Config{}), Tasks: db, Ready: db.Ping},
	)
	if err != nil {
		db.Close()
		return nil, nil, fmt.Errorf("build the web handler: %w", err)
	}
	return handler, db.Close, nil
}

// noSpecs stands in for the team's home when none is configured.
type noSpecs struct{}

func (noSpecs) TaskSpecs(context.Context) ([]domain.SpecFile, error) { return nil, nil }

func specSource(homeDir string) web.SpecSource {
	if homeDir == "" {
		return noSpecs{}
	}
	return yamlfile.NewHome(os.DirFS(homeDir))
}

// yamlParser lets the settings service read documents through the yamlfile adapter.
type yamlParser struct{}

func (yamlParser) Parse(text string) (settings.Parsed, error) {
	doc, secrets, err := yamlfile.ParseSettings([]byte(text))
	if err != nil {
		return settings.Parsed{}, asProblems(err)
	}
	rendered, err := doc.Render()
	if err != nil {
		return settings.Parsed{}, fmt.Errorf("render settings: %w", err)
	}
	out := settings.Parsed{Text: string(rendered), Secrets: secrets, Repositories: doc.Settings().Repositories}
	if out.Text == "{}\n" {
		out.Text = text
	}
	for _, c := range doc.Settings().Connections {
		out.Connections = append(out.Connections, c.Name)
	}
	return out, nil
}

// asProblems turns the adapter's errors into the problems a person reads. Any other error stays as it is.
func asProblems(err error) error {
	var lines yamlfile.ParseErrors
	switch {
	case errors.As(err, &lines):
		problems := make([]domain.SettingsProblem, len(lines))
		for i, l := range lines {
			problems[i] = domain.SettingsProblem{Line: l.Line, Message: plain(l.Msg)}
		}
		return &domain.InvalidSettingsError{Problems: problems}
	case errors.Is(err, yamlfile.ErrInvalid):
		return &domain.InvalidSettingsError{Problems: []domain.SettingsProblem{{Message: plain(err.Error())}}}
	}
	return err
}

// plain drops the error-wrapping prefixes the parsers add, which mean nothing to a person.
func plain(msg string) string {
	for _, prefix := range []string{"invalid file: ", "bad settings: "} {
		msg = strings.TrimPrefix(msg, prefix)
	}
	return msg
}
