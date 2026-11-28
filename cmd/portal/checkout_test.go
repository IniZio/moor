package main

import (
	"context"
	"net/http"
	"testing"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
)

func TestAPullRequestTaskChecksOutItsHeadCommitThroughThePullRequestRef(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/pr-evidence.yaml": evidenceSpec})
	got := r.load(r.hook("d-1", "opened", "5f0e2b19c4d7").Tasks[0])
	if got.Ref != "refs/pull/90/head" || got.Commit != "5f0e2b19c4d7" {
		t.Fatalf("the task checks out %q at %q", got.Ref, got.Commit)
	}
}

func TestAFailedCheckoutFailsTheTaskWithGitsReasonOnItsRunPage(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	m.host.failCheckout = "fatal: unable to access the repository: The requested URL returned error: 403"
	m.run(t, p.addr)
	eventually(t, "the machine linked", func() bool { return p.machines("Mia")[m.id].State == "linked" })
	id := p.startTask("Fix the checkout")
	eventually(t, "the task failed", stateIs(t, db, id, domain.Failed))
	want := "check out github.com/example-org/shop-web: " + m.host.failCheckout
	if got, _ := db.Load(context.Background(), id); got.Failure != want {
		t.Fatalf("failure %q, want %q", got.Failure, want)
	}
	p.do(req{method: http.MethodGet, path: "/tasks/" + string(id), person: "Mia"}).has(t, http.StatusOK, "The requested URL returned error: 403")
}
