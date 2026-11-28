package main

import (
	"context"
	"fmt"
	"strconv"

	"github.com/IniZio/moor/internal/adapter/github"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// publisher is what a publish goes through.
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
// then publishes a result with the line and the address, one version more than the task has.
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

// publishTask publishes a result on a task of the database. It holds no GitHub App, so the answer a pull request's
// task owes stays owing: the portal posts answers.
func publishTask(ctx context.Context, dsn, idText, line, url string) error {
	db, err := store.Open(ctx, dsn)
	if err != nil {
		return fmt.Errorf("open the database: %w", err)
	}
	defer db.Close()
	svc := task.New(db, clock{}, task.Config{Answerable: map[domain.EventSource]bool{github.Source: true}})
	return stage{svc, db, svc}.publishOne(ctx, domain.TaskID(idText), line, url)
}
