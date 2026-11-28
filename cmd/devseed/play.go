package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// clock reads the system time for the service the player drives.
type clock struct{}

func (clock) Now() time.Time { return time.Now().UTC() }

// player plays a task through its life one step at a time, through the task service and the store, so every row
// it writes is one a real runner and the portal would write. A step waits for wait paces before it acts.
type player struct {
	svc       *task.Service
	store     task.Store
	id        domain.TaskID
	repo      domain.Repo
	pace      time.Duration
	resultURL string
	runner    domain.RunnerID
	version   int
}

type step struct {
	wait int
	do   func(context.Context, *player) error
}

// scripts are the sequences a person can play. Each starts a task, then takes it through set up and on.
var scripts = map[string][]step{
	"run":    append(setUp(), publish(6, "Added the wishlist button and checked it in the browser."), end(1)),
	"hold":   append(setUp(), step{wait: 600, do: func(context.Context, *player) error { return nil }}),
	"fail":   append(setUp(), step{wait: 5, do: harnessExits}),
	"review": append(setUp(), publish(6, "Five of five flows passed."), review(1), publish(6, "Four of five flows passed, one not tried."), delivered(2)),
}

func setUp() []step {
	return []step{
		{wait: 2, do: slotTaken},
		{wait: 2, do: note(domain.PhaseDesk, "build-1")},
		{wait: 2, do: note(domain.PhaseBox, "box:dev")},
		{wait: 1, do: note(domain.PhaseSettings, "v1, 3 allow rules, 2 connections")},
		{wait: 1, do: note(domain.PhaseCheckout, "main at 5f0e2b1")},
		{wait: 1, do: note(domain.PhasePrompt, "")},
		{wait: 0, do: partsUp},
	}
}

func slotTaken(ctx context.Context, p *player) error {
	_, err := p.svc.Report(ctx, p.id, task.Report{Kind: task.SlotTaken, Runner: p.runner})
	return wrap("slot taken", err)
}

func partsUp(ctx context.Context, p *player) error {
	_, err := p.svc.Report(ctx, p.id, task.Report{Kind: task.PartsUp, Run: 1})
	return wrap("parts up", err)
}

func harnessExits(ctx context.Context, p *player) error {
	_, err := p.svc.Report(ctx, p.id, task.Report{Kind: task.HarnessExited, Run: 1})
	return wrap("harness exited", err)
}

func note(phase, detail string) func(context.Context, *player) error {
	return func(ctx context.Context, p *player) error { return p.appendNote(ctx, domain.NoteText(phase, detail)) }
}

func review(wait int) step { return step{wait: wait, do: note(domain.PhaseReview, "")} }

func publish(wait int, line string) step {
	return step{wait: wait, do: func(ctx context.Context, p *player) error {
		p.version++
		_, err := p.svc.Publish(ctx, p.id, task.PublishInput{Line: line, EntryURL: p.resultURL, Digest: fmt.Sprintf("v%d-%s", p.version, p.id)})
		return wrap("publish", err)
	}}
}

func end(wait int) step {
	return step{wait: wait, do: func(ctx context.Context, p *player) error {
		_, err := p.svc.End(ctx, p.id)
		return wrap("end", err)
	}}
}

func delivered(wait int) step {
	return step{wait: wait, do: func(ctx context.Context, p *player) error {
		in := task.DeliveredInput{Target: "pull request #90", ExternalID: "c-1", Version: p.version}
		_, err := p.svc.Delivered(ctx, p.id, in)
		return wrap("delivered", err)
	}}
}

// appendNote writes one note row, in a write of its own, since the service has no call for a note.
func (p *player) appendNote(ctx context.Context, text string) error {
	for range 5 {
		t, err := p.store.Load(ctx, p.id)
		if err != nil {
			return wrap("load task", err)
		}
		data, err := json.Marshal(domain.NoteData{Text: text})
		if err != nil {
			return wrap("encode note", err)
		}
		t.LastSeq++
		row := domain.EventRow{Task: p.id, Seq: t.LastSeq, At: time.Now().UTC(), Kind: domain.KindNote, Data: data}
		if _, err = p.store.Save(ctx, domain.TaskWrite{Task: t, Events: []domain.EventRow{row}}); !errors.Is(err, domain.ErrStaleVersion) {
			return wrap("save note", err)
		}
	}
	return errors.New("save note: another writer kept getting in first")
}

func wrap(what string, err error) error {
	if err != nil {
		return fmt.Errorf("%s: %w", what, err)
	}
	return nil
}

// play runs the named script on the task, pausing between steps.
func (p *player) play(ctx context.Context, name string) error {
	steps, ok := scripts[name]
	if !ok {
		return fmt.Errorf("no script %q, the scripts are run, hold, fail and review", name)
	}
	p.version = 0
	for _, s := range steps {
		select {
		case <-ctx.Done():
			return fmt.Errorf("stopped: %w", ctx.Err())
		case <-time.After(time.Duration(s.wait) * p.pace):
		}
		if err := s.do(ctx, p); err != nil {
			return err
		}
	}
	return nil
}

// start makes the task a script plays: a person's for most, and for review one that a pull request event started,
// with a review prompt, so the Review and Answer parts have something to show.
func (p *player) start(ctx context.Context, name string, by string, prompt string) error {
	if name != "review" {
		_, err := p.svc.StartByPerson(ctx, task.StartInput{ID: p.id, Repo: p.repo, Prompt: prompt, By: domain.Person(by)})
		return wrap("start", err)
	}
	_, err := p.svc.StartByEvent(ctx, task.EventInput{
		ID: p.id, Repo: p.repo, Spec: domain.Spec{Name: "PR evidence", File: "tasks/pr-evidence.yaml"},
		Event:  domain.Event{Source: "github", Group: domain.EventGroup("pr-" + string(p.id)), Rev: domain.Revision("rev-" + string(p.id))},
		Prompt: prompt, ReviewPrompt: "Check your page against the check list.\nFix what fails, then publish again.", CancelInProgress: true,
	})
	return wrap("start", err)
}
