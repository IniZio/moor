// Command devseed fills a development database with synthetic tasks in every state, through the store, so a
// screen can be looked at before a runner is connected. It adds to what is there and never reads credentials.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"strconv"
	"time"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func main() {
	dsn := flag.String("database-url", os.Getenv("PORTAL_DATABASE_URL"), "Postgres URL (PORTAL_DATABASE_URL)")
	playName := flag.String("play", "", "start one task and play it over time instead of seeding: run, hold, fail or review")
	pace := flag.Duration("pace", 2*time.Second, "how long one wait of a played script lasts")
	resultURL := flag.String("result-url", "https://results.example/page", "address a played task publishes as its result page")
	repo := flag.String("repo", "example-org/shop-web", "repository of a played task, which Settings must allow for its page to be useful")
	publish := flag.String("publish", "", "publish a result on the task with this number, as a runner and an agent would, and answer its pull request when a GitHub App is given")
	line := flag.String("line", "Five of five flows passed.", "the one line of the result published with -publish")
	var app github.Config
	flag.Int64Var(&app.AppID, "github-app-id", envInt("PORTAL_GITHUB_APP_ID"), "GitHub App id for -publish (PORTAL_GITHUB_APP_ID)")
	flag.StringVar(&app.KeyFile, "github-private-key-file", os.Getenv("PORTAL_GITHUB_PRIVATE_KEY_FILE"), "the App's private key file for -publish (PORTAL_GITHUB_PRIVATE_KEY_FILE)")
	flag.StringVar(&app.APIBase, "github-api-url", os.Getenv("PORTAL_GITHUB_API_URL"), "GitHub's API address for -publish (PORTAL_GITHUB_API_URL)")
	flag.Parse()
	app.WebhookSecret = []byte(os.Getenv("PORTAL_GITHUB_WEBHOOK_SECRET"))
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	var err error
	switch {
	case *publish != "":
		err = publishTask(ctx, *dsn, app, *publish, *line, *resultURL)
	case *playName != "":
		err = playOne(ctx, *dsn, *playName, *repo, *pace, *resultURL)
	default:
		err = run(ctx, *dsn, time.Now().UTC())
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "devseed:", err)
		os.Exit(1)
	}
}

// playOne starts a task and plays a script on it, printing the task's number first so a caller can open its page.
func playOne(ctx context.Context, dsn, name, repoText string, pace time.Duration, resultURL string) error {
	repo, err := domain.ParseRepo(repoText)
	if err != nil {
		return fmt.Errorf("repository: %w", err)
	}
	db, err := store.Open(ctx, dsn)
	if err != nil {
		return fmt.Errorf("open the database: %w", err)
	}
	defer db.Close()
	id, err := db.NextTaskID(ctx)
	if err != nil {
		return fmt.Errorf("task number: %w", err)
	}
	p := &player{
		svc:   task.New(db, clock{}, task.Config{Answerable: map[domain.EventSource]bool{"github": true}}),
		store: db, id: id, repo: repo, pace: pace, resultURL: resultURL, runner: "build-1",
	}
	if err := p.start(ctx, name, "Mia", "Add a wishlist button to the product page and keep it after sign-in"); err != nil {
		return err
	}
	fmt.Println(id)
	return p.play(ctx, name)
}

type seed struct {
	by        string
	repo      string
	prompt    string
	spec      string
	source    string
	state     domain.State
	ago       time.Duration
	lasted    time.Duration
	supersede string
}

var seeds = []seed{
	{by: "Dana", repo: "example-org/shop-web", prompt: "Add a wishlist button to the product page and keep it after sign-in", state: domain.Running, ago: 4 * time.Minute},
	{by: "Eli", repo: "example-org/docs-site", prompt: "Replace the outdated screenshots in the install guide", state: domain.Waiting, ago: time.Minute},
	{spec: "PR evidence", source: "github", repo: "example-org/shop-web", state: domain.Done, ago: 12 * time.Minute, lasted: 9*time.Minute + 41*time.Second, supersede: "pr-90"},
	{spec: "PR evidence", source: "github", repo: "example-org/shop-web", state: domain.Superseded, ago: 13 * time.Minute, lasted: 40 * time.Second, supersede: "pr-90"},
	{by: "Fay", repo: "example-org/shop-web", prompt: "Upgrade the date library and check the order pages still render", state: domain.Failed, ago: 40 * time.Minute, lasted: 3*time.Minute + 10*time.Second},
	{by: "Eli", repo: "example-org/docs-site", prompt: "Draft a changelog entry for the new search", state: domain.Done, ago: 2 * time.Hour, lasted: 7*time.Minute + 55*time.Second},
	{by: "Dana", repo: "example-org/shop-api", prompt: "Cancelled experiment with the queue", state: domain.Cancelled, ago: 5 * time.Hour, lasted: 20 * time.Second},
	{spec: "PR evidence", by: "Eli", repo: "example-org/docs-site", state: domain.Done, ago: 26 * time.Hour, lasted: 11*time.Minute + 20*time.Second, supersede: "pr-41"},
	{spec: "Dependency check", source: "schedule", repo: "example-org/shop-web", state: domain.Done, ago: 4 * 24 * time.Hour, lasted: 5*time.Minute + 48*time.Second},
	{spec: "Dependency check", source: "schedule", repo: "example-org/shop-api", state: domain.Done, ago: 4 * 24 * time.Hour, lasted: 4*time.Minute + 9*time.Second},
}

func run(ctx context.Context, dsn string, now time.Time) error {
	db, err := store.Open(ctx, dsn)
	if err != nil {
		return fmt.Errorf("open the database: %w", err)
	}
	defer db.Close()
	for i := len(seeds) - 1; i >= 0; i-- {
		id, err := db.NextTaskID(ctx)
		if err != nil {
			return fmt.Errorf("task number: %w", err)
		}
		t, err := seeds[i].task(id, now)
		if err != nil {
			return err
		}
		note := domain.EventRow{Task: id, Seq: 1, At: t.CreatedAt, Kind: domain.KindNote, Data: json.RawMessage(`{"text":"seeded"}`)}
		if _, err := db.Save(ctx, domain.TaskWrite{Task: t, Create: true, Events: []domain.EventRow{note}}); err != nil {
			return fmt.Errorf("seed task %s: %w", id, err)
		}
	}
	return nil
}

func (s seed) task(id domain.TaskID, now time.Time) (domain.Task, error) {
	repo, err := domain.ParseRepo(s.repo)
	if err != nil {
		return domain.Task{}, fmt.Errorf("seed repository: %w", err)
	}
	t := domain.Task{ID: id, Repo: repo, Prompt: s.prompt, StartedBy: s.by, State: s.state, LastSeq: 1, CreatedAt: now.Add(-s.ago)}
	if s.lasted > 0 {
		t.EndedAt = t.CreatedAt.Add(s.lasted)
	}
	if s.spec == "" {
		return t, nil
	}
	t.Prompt = "Run the " + s.spec + " spec."
	t.Spec = &domain.Spec{Name: domain.SpecName(s.spec), File: "tasks/" + s.spec + ".yaml"}
	group := s.supersede
	if group == "" {
		group = "repo-" + string(repo.Path)
	}
	t.Event = &domain.Event{Source: domain.EventSource(s.source), Group: domain.EventGroup(group), Rev: domain.Revision("rev-" + string(id))}
	if s.by == "" {
		t.StartedBy = domain.EventStarterPrefix + s.source
	}
	return t, nil
}

// envInt reads a whole number from the environment, and gives 0 for text that is not one.
func envInt(name string) int64 {
	n, err := strconv.ParseInt(os.Getenv(name), 10, 64)
	if err != nil {
		return 0
	}
	return n
}
