package store

import (
	"context"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

func storeWithTasks(t *testing.T, ids ...string) *Store {
	t.Helper()
	s := freshStore(t)
	for _, id := range ids {
		if _, err := s.Save(context.Background(), domain.TaskWrite{Task: newTask(id, domain.Revision("r"+id)), Create: true, Events: []domain.EventRow{{Task: domain.TaskID(id), Seq: 1, At: time.Unix(1, 0), Kind: domain.KindNote}}}); err != nil {
			t.Fatal(err)
		}
	}
	return s
}

func TestATasksDefaultBranchIsKeptFromItsFirstRead(t *testing.T) {
	s, ctx := storeWithTasks(t, "1"), context.Background()
	if _, read, err := s.DefaultBranch(ctx, "1"); err != nil || read {
		t.Fatalf("before any read: %v, %v", read, err)
	}
	for _, b := range []string{"trunk", "main"} {
		if err := s.SetDefaultBranch(ctx, "1", b); err != nil {
			t.Fatal(err)
		}
	}
	if got, read, err := s.DefaultBranch(ctx, "1"); err != nil || !read || got != "trunk" {
		t.Fatalf("after two reads: %q, %v, %v", got, read, err)
	}
}

func TestADefaultBranchThatCouldNotBeReadIsKeptAsRead(t *testing.T) {
	s, ctx := storeWithTasks(t, "2"), context.Background()
	if err := s.SetDefaultBranch(ctx, "2", ""); err != nil {
		t.Fatal(err)
	}
	if got, read, err := s.DefaultBranch(ctx, "2"); err != nil || !read || got != "" {
		t.Fatalf("a branch that could not be read: %q, %v, %v", got, read, err)
	}
}
