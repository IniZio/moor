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
	"github.com/IniZio/moor/internal/answer"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/inbound"
	"github.com/IniZio/moor/internal/settings"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/web"
)

type systemClock struct{}

func (systemClock) Now() time.Time { return time.Now().UTC() }

// portal is the assembled product. Answers posts a published result back to its pull request. The route that
// publishes a result calls it, and no such route is served yet.
type portal struct {
	handler http.Handler
	answers *answer.Service
	close   func()
}

// build assembles the portal and gives its handler and what closes it.
func build(ctx context.Context, cfg config) (http.Handler, func(), error) {
	p, err := assemble(ctx, cfg)
	if err != nil {
		return nil, nil, err
	}
	return p.handler, p.close, nil
}

// assemble opens the store and wires the services into the web handler. cmd is the one place that knows every adapter.
func assemble(ctx context.Context, cfg config) (*portal, error) {
	db, err := store.Open(ctx, cfg.database)
	if err != nil {
		return nil, fmt.Errorf("open the database: %w", err)
	}
	box, err := secretbox.New(cfg.secretKey)
	if err != nil {
		db.Close()
		return nil, fmt.Errorf("secret key: %w", err)
	}
	hub := &githubApp{}
	svc := settings.New(db, yamlParser{}, box, systemClock{}, hub)
	hub.svc = svc
	taskCfg := withKeep(taskConfig(), cfg)
	tasks := task.New(db, systemClock{}, taskCfg)
	rn := newRunners(ctx, cfg, db, tasks, svc)
	specs := specSource(cfg.homeDir)
	events := inbound.New(inbound.Deps{
		Store: txStore{db}, Numbers: db, Tasks: tasks, Specs: specs, Allowed: svc, Link: links{hub},
		Clock: systemClock{}, Config: taskCfg, Stop: rn.control.Stop,
	})
	handler, err := web.New(
		web.Config{UserHeader: cfg.userHeader, OrgName: cfg.orgName, TrustedOrigins: cfg.trustedOrigins, InternalSecret: cfg.internalSecret},
		web.Deps{
			Settings: svc, Specs: specs, Start: tasks, Act: acts{tasks, rn.control}, Tasks: db,
			Machines: rn.machines, Terms: terms{rn.control}, Ready: db.Ping, Run: events, Events: readers(),
			Proxies: newProxyAPI(cfg, svc, db), Blocked: svc,
			Results: newResults(cfg, db, hub.answers(db, tasks)),
		},
	)
	if err != nil {
		db.Close()
		return nil, fmt.Errorf("build the web handler: %w", err)
	}
	return &portal{handler: hub.mount(rn.mount(handler), events), answers: hub.answers(db, tasks), close: func() { rn.close(); db.Close() }}, nil
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
	out.Settings = doc.Settings()
	if out.Text == "{}\n" {
		out.Text = text
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
