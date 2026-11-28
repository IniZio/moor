package store

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestATaskStartedFromASpecKeepsItsHomeCommitAsTheFirstOfItsCommits(t *testing.T) {
	ctx := context.Background()
	s := freshStore(t)
	withCommit := newTask("1", "r1")
	withCommit.HomeCommit = "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567"
	mustCreate(t, s, withCommit)
	mustCreate(t, s, newTask("2", "r2"))

	got, err := s.Load(ctx, "1")
	if err != nil || got.HomeCommit != withCommit.HomeCommit {
		t.Fatalf("home commit %q, %v", got.HomeCommit, err)
	}
	if other, _ := s.Load(ctx, "2"); other.HomeCommit != "" {
		t.Fatalf("a task with no home commit read %q", other.HomeCommit)
	}
	got.State = domain.Starting
	saved, err := s.Save(ctx, domain.TaskWrite{Task: got})
	if err != nil || saved.HomeCommit != withCommit.HomeCommit {
		t.Fatalf("after an update the home commit is %q, %v", saved.HomeCommit, err)
	}
	if n := count(t, s, "SELECT count(*) FROM task_home_commits WHERE task_id = '1' AND n = 1 AND cause = 'start'"); n != 1 {
		t.Fatalf("%d start rows", n)
	}
	if n := count(t, s, "SELECT count(*) FROM task_home_commits WHERE task_id = '2'"); n != 0 {
		t.Fatalf("%d rows for a task with no home commit", n)
	}
}
