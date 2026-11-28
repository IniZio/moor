package main

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

func TestStartBringsThePartsUpAndTheSessionLive(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	lines := watch(t, g.ctx, s)
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	await(t, g.ctx, lines, "the desk running", partIs("t1", "desk", "running"))
	l := await(t, g.ctx, lines, "the session live", sessionIs("t1", "live"))
	if l.Fence != (wire.Fence{Task: "t1", Run: 1, Gen: 1}) {
		t.Fatalf("session line %+v carries another launch", l)
	}
	terms := g.desks.core("t1").Terminals()
	if len(terms) != 1 || *terms[0].Name != sessionName || terms[0].Labels["moor.run"] != "1" {
		t.Fatalf("desk terminals %+v, want one session of run 1", terms)
	}
}

func TestARepeatedStartJoinsAndAnOlderOneIsRefused(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	for range 2 {
		if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 2, 1), nil); err != nil {
			t.Fatal(err)
		}
	}
	if g.host.ups != 1 || len(g.desks.core("t1").Terminals()) != 1 {
		t.Fatalf("%d ups and %d sessions, want one of each", g.host.ups, len(g.desks.core("t1").Terminals()))
	}
	err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil)
	if !errors.Is(err, &wire.Error{Code: wire.CodeRefused}) {
		t.Fatalf("an older run must be refused: %v", err)
	}
}

func TestStopIsFencedAndSucceedsForAnUnknownTask(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	if err := call(g.ctx, s, wire.KindTaskStop, wire.TaskStop{Fence: wire.Fence{Task: "nope", Run: 1}}, nil); err != nil {
		t.Fatalf("a stop for a task the host does not have succeeds: %v", err)
	}
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 2, 1), nil); err != nil {
		t.Fatal(err)
	}
	lines := watch(t, g.ctx, s)
	if err := call(g.ctx, s, wire.KindTaskStop, wire.TaskStop{Fence: wire.Fence{Task: "t1", Run: 1, Gen: 1}}, nil); err != nil {
		t.Fatal(err)
	}
	if held, _ := g.host.List(g.ctx); len(held) != 1 {
		t.Fatal("a stop with an older run must leave the task running")
	}
	if err := call(g.ctx, s, wire.KindTaskStop, wire.TaskStop{Fence: wire.Fence{Task: "t1", Run: 2, Gen: 1}}, nil); err != nil {
		t.Fatal(err)
	}
	await(t, g.ctx, lines, "the box removed", partIs("t1", "box", runner.Removed))
	if held, _ := g.host.List(g.ctx); len(held) != 0 {
		t.Fatalf("after the stop the host holds %+v", held)
	}
	if err := call(g.ctx, s, wire.KindTaskRelease, wire.TaskRelease{Task: "t1"}, nil); err != nil {
		t.Fatalf("releasing a removed task again succeeds: %v", err)
	}
}

func TestARestartedRunnerReadoptsByLabel(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	var before wire.TaskListReply
	if err := call(g.ctx, s, wire.KindTaskList, wire.TaskList{}, &before); err != nil {
		t.Fatal(err)
	}
	stop()

	s, stop = g.startFake()
	defer stop()
	var after wire.TaskListReply
	if err := call(g.ctx, s, wire.KindTaskList, wire.TaskList{}, &after); err != nil {
		t.Fatal(err)
	}
	if len(after.Tasks) != 1 || after.Tasks[0].Fence != before.Tasks[0].Fence || len(after.Tasks[0].Containers) != len(partNames) {
		t.Fatalf("after the restart %+v, before %+v", after, before)
	}
	lines := watch(t, g.ctx, s)
	await(t, g.ctx, lines, "the adopted session live", sessionIs("t1", "live"))
	g.desks.core("t1").Exit(sessionName, 3)
	if l := await(t, g.ctx, lines, "the session finished", sessionIs("t1", "finished")); l.Code != 3 {
		t.Fatalf("finished with %d, want the harness's 3", l.Code)
	}
	if n := len(g.desks.core("t1").Terminals()); n != 1 {
		t.Fatalf("re-adoption started %d sessions, want the one already there", n)
	}
}

func TestTheRunnerRelinksAfterTheLinkDrops(t *testing.T) {
	g := newRig(t)
	_, stop := g.startFake()
	defer stop()
	g.portal.Drop()
	s, err := g.portal.Next(g.ctx)
	if err != nil {
		t.Fatal(err)
	}
	var st wire.RunnerStatusReply
	if err := call(g.ctx, s, wire.KindRunnerStatus, wire.RunnerStatus{}, &st); err != nil || st.Proto != wire.Proto {
		t.Fatalf("status over the new link: %+v, %v", st, err)
	}
}

func TestAnUnknownOpAndABadBodyAreAnswered(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	defer stop()
	if err := call(g.ctx, s, wire.KindWarmBuild, wire.WarmBuild{}, nil); !errors.Is(err, &wire.Error{Code: wire.CodeUnknownOp}) {
		t.Fatalf("an op the runner does not serve: %v", err)
	}
	bad := map[string]any{"task": "t1", "run": 1, "gen": 1, "surprise": true}
	if err := call(g.ctx, s, wire.KindTaskStart, bad, nil); !errors.Is(err, &wire.Error{Code: wire.CodeBadRequest}) {
		t.Fatalf("a start with an unknown field: %v", err)
	}
}

func TestARefusedRunnerStopsDialling(t *testing.T) {
	g := newRig(t)
	agent := g.portal.Agent()
	_, agent.Key, _ = ed25519.GenerateKey(rand.Reader)
	r := newRunner(agent, g.host, sessions{connect: g.desks.connect})
	ctx, cancel := context.WithCancel(g.ctx)
	defer cancel()
	if err := r.Run(ctx); !errors.Is(err, runner.ErrFatal) {
		t.Fatalf("a refused runner ends with ErrFatal: %v", err)
	}
}

func TestARevokedRunnerRemovesTheContainersOfTheTasksItHoldsAndExits(t *testing.T) {
	g := newRig(t)
	r := newRunner(g.portal.Agent(), g.host, sessions{connect: g.desks.connect, attach: g.ttys.attach, clear: g.cleared.clear})
	done := make(chan error, 1)
	go func() { done <- r.Run(g.ctx) }()
	s, err := g.portal.Next(g.ctx)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"t1", "t2"} {
		if err := call(g.ctx, s, wire.KindTaskStart, startMsg(id, 1, 1), nil); err != nil {
			t.Fatal(err)
		}
	}
	if held, _ := g.host.List(g.ctx); len(held) != 2 {
		t.Fatalf("the host holds %d tasks before the revoke, want 2", len(held))
	}
	g.portal.Revoke()
	g.portal.Drop()
	select {
	case err := <-done:
		if !errors.Is(err, runner.ErrRevoked) || !errors.Is(err, runner.ErrFatal) {
			t.Fatalf("the revoked runner ended with %v, want ErrRevoked", err)
		}
	case <-g.ctx.Done():
		t.Fatal("the revoked runner kept running")
	}
	if held, _ := g.host.List(g.ctx); len(held) != 0 {
		t.Fatalf("a revoked runner left %+v on the host", held)
	}
}

func TestAnUnknownRunnerIsRefusedAndTouchesNothing(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	stop()
	agent := g.portal.Agent()
	_, agent.Key, _ = ed25519.GenerateKey(rand.Reader)
	if err := newRunner(agent, g.host, sessions{connect: g.desks.connect}).Run(g.ctx); !errors.Is(err, runner.ErrFatal) || errors.Is(err, runner.ErrRevoked) {
		t.Fatalf("a runner with the wrong key ends with %v, want ErrFatal and not ErrRevoked", err)
	}
	if held, _ := g.host.List(g.ctx); len(held) != 1 {
		t.Fatalf("a refusal that is not a revoke removed tasks: %+v", held)
	}
}

func TestAdoptionClearsTheViewersAStaleRunnerLeftInEachDesk(t *testing.T) {
	g := newRig(t)
	s, stop := g.startFake()
	for _, id := range []string{"t1", "t2"} {
		if err := call(g.ctx, s, wire.KindTaskStart, startMsg(id, 1, 1), nil); err != nil {
			t.Fatal(err)
		}
	}
	stop()
	for len(g.cleared.calls) > 0 {
		<-g.cleared.calls
	}
	_, stop = g.startFake()
	defer stop()
	got := map[string]bool{}
	for len(got) < 2 {
		select {
		case id := <-g.cleared.calls:
			got[id] = true
		case <-g.ctx.Done():
			t.Fatalf("the new runner cleared the viewers of %v, want t1 and t2", got)
		}
	}
}
