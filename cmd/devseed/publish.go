package main

import (
	"context"
	"fmt"
	"strconv"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/answer"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// publisher is what a publish goes through: the answer service, which posts what the publish leaves owing.
type publisher interface {
	Publish(ctx context.Context, id domain.TaskID, in task.PublishInput) (task.Outcome, error)
}

// stage is what publishOne works through: the task service, the store it reads, and the publisher.
type stage struct {
	svc   *task.Service
	reads task.Store
	pub   publisher
}

// publishOne stands in for a runner and an agent on a task a webhook started: it takes a waiting task up to running,
// then publishes a result with the line and the address, one version more than the task has. A task that answers its
// event is answered, so the comment on its pull request is made, or edited when it was made before.
func (s stage) publishOne(ctx context.Context, id domain.TaskID, line, url string) error {
	svc, reads, pub := s.svc, s.reads, s.pub
	t, err := reads.Load(ctx, id)
	if err != nil {
		return fmt.Errorf("load task %s: %w", id, err)
	}
	if t.State == domain.Waiting {
		if _, err := svc.Report(ctx, id, task.Report{Kind: task.SlotTaken, Runner: "build-1"}); err != nil {
			return fmt.Errorf("slot taken: %w", err)
		}
		if _, err := svc.Report(ctx, id, task.Report{Kind: task.PartsUp, Run: 1}); err != nil {
			return fmt.Errorf("parts up: %w", err)
		}
	}
	versions, err := reads.Versions(ctx, id)
	if err != nil {
		return fmt.Errorf("versions of %s: %w", id, err)
	}
	digest := "dev-" + string(id) + "-v" + strconv.Itoa(len(versions)+1)
	if _, err := pub.Publish(ctx, id, task.PublishInput{Line: line, EntryURL: url, Digest: digest}); err != nil {
		return fmt.Errorf("publish: %w", err)
	}
	return nil
}

// publishTask publishes a result on a task of the database, answering through the GitHub App when one is given.
func publishTask(ctx context.Context, dsn string, app github.Config, idText, line, url string) error {
	db, err := store.Open(ctx, dsn)
	if err != nil {
		return fmt.Errorf("open the database: %w", err)
	}
	defer db.Close()
	cfg := task.Config{Answerable: map[domain.EventSource]bool{github.Source: true}}
	svc := task.New(db, clock{}, cfg)
	posters := map[domain.EventSource]answer.Poster{}
	if app.Enabled() {
		a, err := github.New(app)
		if err != nil {
			return fmt.Errorf("github app: %w", err)
		}
		posters[github.Source] = a
	}
	pub := answer.New(answer.Deps{Tasks: svc, Reads: db, Deliveries: db, Posters: posters, Clock: clock{}})
	return stage{svc, db, pub}.publishOne(ctx, domain.TaskID(idText), line, url)
}
