package main

import (
	"context"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
)

func playOnMemory(t *testing.T, name string) (domain.Task, []domain.EventRow, *tasktest.Memory) {
	t.Helper()
	mem := tasktest.NewMemory()
	p := &player{
		svc:   task.New(mem, clock{}, task.Config{Answerable: map[domain.EventSource]bool{"github": true}}),
		store: mem, id: "7", repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, pace: time.Microsecond,
		resultURL: "https://results.example/page", runner: "build-1",
	}
	ctx := context.Background()
	if err := p.start(ctx, name, "Mia", "Add a button"); err != nil {
		t.Fatal(err)
	}
	if err := p.play(ctx, name); err != nil {
		t.Fatal(err)
	}
	got, err := mem.Load(ctx, "7")
	if err != nil {
		t.Fatal(err)
	}
	return got, mem.Events("7"), mem
}

func kinds(rows []domain.EventRow) map[domain.EventKind]int {
	out := map[domain.EventKind]int{}
	for _, r := range rows {
		out[r.Kind]++
	}
	return out
}

func TestEachScriptEndsWhereItSays(t *testing.T) {
	notes := map[string]int{"run": 5, "fail": 5, "hold": 5, "review": 6}
	for name, want := range map[string]domain.State{"run": domain.Done, "fail": domain.Failed, "hold": domain.Running, "review": domain.Done} {
		got, rows, _ := playOnMemory(t, name)
		if got.State != want {
			t.Errorf("%s ended %s, want %s", name, got.State, want)
		}
		if got.LastSeq != int64(len(rows)) {
			t.Errorf("%s: last_seq %d, %d rows", name, got.LastSeq, len(rows))
		}
		if k := kinds(rows); k[domain.KindNote] != notes[name] || k[domain.KindState] < 3 {
			t.Errorf("%s wrote %v", name, k)
		}
	}
}

func TestAReviewScriptPublishesTwiceAndAnswersOnce(t *testing.T) {
	got, rows, mem := playOnMemory(t, "review")
	k := kinds(rows)
	if k[domain.KindPublished] != 2 || k[domain.KindDelivered] != 1 || got.Event == nil || got.Spec == nil || got.ReviewPrompt == "" {
		t.Errorf("rows %v, event %v spec %v review %q", k, got.Event, got.Spec, got.ReviewPrompt)
	}
	versions, err := mem.Versions(context.Background(), "7")
	if err != nil || len(versions) != 2 {
		t.Errorf("versions %v, err %v", versions, err)
	}
}

func TestAnUnknownScriptIsRefusedWithTheNamesThatExist(t *testing.T) {
	p := &player{pace: time.Microsecond}
	if err := p.play(context.Background(), "nope"); err == nil {
		t.Fatal("an unknown script was played")
	}
}
