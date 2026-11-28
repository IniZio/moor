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
	events  *inbound.Service
	close   func()
}

// build assembles the portal, starts its clock, which fires the specs' schedules, and gives its handler and what
// stops the clock and closes it.
func build(ctx context.Context, cfg config) (http.Handler, func(), error) {
	p, err := assemble(ctx, cfg)
	if err != nil {
		return nil, nil, err
	}
	clock, stop := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		p.events.Loop(clock, wallClock{})
	}()
	return p.handler, func() { stop(); <-done; p.close() }, nil
}

// wallClock is the clock the schedules fire on.
type wallClock struct{ systemClock }

// Wait sleeps for d, and says false when ctx ends first.
func (wallClock) Wait(ctx context.Context, d time.Duration) bool {
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-t.C:
		return true
	}
}

// assemble opens the store and wires the services into the web handler. cmd is the one place that knows every adapter.
func assemble(ctx context.Context, cfg config) (*portal, error) {
	db, err := store.OpenIn(ctx, cfg.database, cfg.databaseSchema)
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
	if err := seedSettings(ctx, svc, cfg.seedSettingsFile); err != nil {
		db.Close()
		return nil, err
	}
	taskCfg := withOperations(taskConfig(), cfg)
	tasks := task.New(db, systemClock{}, taskCfg)
	rn := newRunners(ctx, cfg, db, tasks, svc)
	specs, closeHome := specSource(ctx, cfg, hub)
	events := inbound.New(inbound.Deps{
		Store: txStore{db}, Numbers: db, Tasks: tasks, Specs: specs, Allowed: svc, Link: links{hub}, Crons: yamlfile.Crons{}, Branches: hub,
		Clock: systemClock{}, Config: taskCfg, Stop: rn.control.Stop,
	})
	handler, err := web.New(
		web.Config{UserHeader: cfg.userHeader, OrgName: cfg.orgName, TrustedOrigins: cfg.trustedOrigins, InternalSecret: cfg.internalSecret},
		web.Deps{
			Settings: svc, Specs: specs, Start: tasks, Act: acts{tasks, rn.control}, Tasks: db,
			Machines: rn.machines, Terms: terms{rn.control}, Run: events, Events: readers(),
			Proxies: newProxyAPI(cfg, svc, db, hub), Blocked: svc,
			Results: newResults(cfg, db, hub.answers(db, tasks)),
		},
	)
	if err != nil {
		closeHome()
		db.Close()
		return nil, fmt.Errorf("build the web handler: %w", err)
	}
	return &portal{handler: hub.mount(rn.mount(handler), events), answers: hub.answers(db, tasks), events: events, close: func() { closeHome(); rn.close(); db.Close() }}, nil
}

// noSpecs stands in for the team's home when none is configured.
type noSpecs struct{}

func (noSpecs) TaskSpecs(context.Context) ([]domain.SpecFile, error) { return nil, nil }

// specSource is the team's home: fetched from its repository, or read from a folder, or none. It gives what ends it.
func specSource(ctx context.Context, cfg config, hub *githubApp) (web.SpecSource, func()) {
	switch {
	case cfg.home.repo.Complete():
		home := newHomeRepo(cfg.home, hub)
		hub.pushes = home
		home.start(ctx, cfg.home.every)
		return home, home.close
	case cfg.homeDir != "":
		return yamlfile.NewHome(os.DirFS(cfg.homeDir)), func() {}
	}
	return noSpecs{}, func() {}
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
