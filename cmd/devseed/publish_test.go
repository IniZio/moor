package main

import (
	"context"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
)

func TestPublishingTakesAWaitingTaskToRunningAndNumbersEachVersion(t *testing.T) {
	mem := tasktest.NewMemory()
	svc := task.New(mem, clock{}, task.Config{})
	ctx := context.Background()
	repo := domain.Repo{Host: "github.com", Path: "example-org/shop-web"}
	if _, err := svc.StartByPerson(ctx, task.StartInput{ID: "7", Repo: repo, Prompt: "x", By: "Mia"}); err != nil {
		t.Fatal(err)
	}
	time.Sleep(time.Millisecond)
	s := stage{svc, mem, svc}
	if err := s.publishOne(ctx, "7", "all passed", "https://results.example/7"); err != nil {
		t.Fatal(err)
	}
	got, err := mem.Load(ctx, "7")
	if err != nil || got.State != domain.Done {
		t.Fatalf("task %+v, %v", got, err)
	}
	versions, _ := mem.Versions(ctx, "7")
	if len(versions) != 1 || versions[0].Digest != "dev-7-v1" || versions[0].Line != "all passed" {
		t.Errorf("versions = %+v", versions)
	}
}
