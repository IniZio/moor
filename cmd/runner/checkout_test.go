package main

import (
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

func TestAPersonStartedTaskChecksOutTheDefaultBranchBeforeItsSessionStarts(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	want := runner.Checkout{Task: "t1", URL: "https://github.com/example-org/shop-web.git", Host: "github.com"}
	if got := g.host.asked(); len(got) != 1 || got[0] != want {
		t.Fatalf("checkouts %+v, want one of %+v", got, want)
	}
}

func TestAPullRequestTaskFetchesItsRefAndChecksOutItsHeadCommit(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	lines := watch(t, g.ctx, s)
	msg := startMsg("t1", 1, 1)
	msg.Ref, msg.Commit = "refs/pull/7/head", "c0ffee"
	if err := call(g.ctx, s, wire.KindTaskStart, msg, nil); err != nil {
		t.Fatal(err)
	}
	if got := g.host.asked(); len(got) != 1 || got[0].Ref != "refs/pull/7/head" || got[0].Commit != "c0ffee" {
		t.Fatalf("checkouts %+v", got)
	}
	await(t, g.ctx, lines, "the checkout note", noteIs("t1", domain.NoteText(domain.PhaseCheckout, "pull/7/head at c0ffee")))
}

func TestAFailedCheckoutFailsTheStartWithGitsReasonAndStartsNoSession(t *testing.T) {
	g := newRig(t)
	g.host.failCheckout = "fatal: unable to access the repository: The requested URL returned error: 403"
	s, stop := g.startFake()
	defer stop()
	err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil)
	if err == nil || !strings.Contains(err.Error(), "check out github.com/example-org/shop-web: "+g.host.failCheckout) {
		t.Fatalf("the start answered %v", err)
	}
	if terms := g.desks.core("t1").Terminals(); len(terms) != 0 {
		t.Fatalf("a session started after the failed checkout: %+v", terms)
	}
}

func TestTheCloneURLTheOperatorSetNamesTheTasksRepository(t *testing.T) {
	g := newRig(t)
	r := newRunner(g.portal.Agent(), g.host, sessions{connect: g.desks.connect, attach: g.ttys.attach, clear: g.clear, argv: []string{"harness"}},
		"http://git.example.test/{host}/{path}.git")
	done := make(chan error, 1)
	go func() { done <- r.Run(g.ctx) }()
	s, err := g.portal.Next(g.ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	if got := g.host.asked(); len(got) != 1 || got[0].URL != "http://git.example.test/github.com/example-org/shop-web.git" {
		t.Fatalf("checkouts %+v", got)
	}
}
