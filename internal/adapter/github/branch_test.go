package github_test

import (
	"context"
	"testing"
)

func TestTheDefaultBranchIsReadFromTheRepository(t *testing.T) {
	r := newRig(t)
	r.fake.SetDefaultBranch("example-org/shop-web", "trunk")
	if got, err := r.adapter.DefaultBranch(context.Background(), "example-org/shop-web"); err != nil || got != "trunk" {
		t.Fatalf("got %q, %v", got, err)
	}
	if got, err := r.adapter.DefaultBranch(context.Background(), "example-org/docs-site"); err != nil || got != "main" {
		t.Fatalf("a repository the fake has no branch for: %q, %v", got, err)
	}
	if _, err := r.adapter.DefaultBranch(context.Background(), "not-a-repo"); err == nil {
		t.Fatal("a name that is not owner/name was read")
	}
}
