package main

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

func TestAJoinedRunnerTakesAWaitingTaskAndItsSessionEndDecidesTheState(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	eventually(t, "the machine linked", func() bool { return p.machines("Mia")[m.id].State == "linked" })
	id := p.startTask("Fix the checkout")
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	got, _ := db.Load(context.Background(), id)
	if got.Runner != domain.RunnerID(m.id) || got.Run != 1 || !m.host.holds(string(id)) || m.host.launches(string(id)) != 1 {
		t.Fatalf("running task %+v, host holds it %v, launched %d times", got, m.host.holds(string(id)), m.host.launches(string(id)))
	}
	if row := p.machines("Mia")[m.id]; row.Running != 1 {
		t.Errorf("machine row %+v, want one running task", row)
	}
	close(m.sessions.end(string(id)))
	eventually(t, "the task failed on its session end", stateIs(t, db, id, domain.Failed))
	if got, _ := db.Load(context.Background(), id); got.Failure != "session ended with nothing published" {
		t.Errorf("failure %q", got.Failure)
	}
}

func TestACancelOnAnotherReplicaReachesTheRunner(t *testing.T) {
	dsn := pgtest.SchemaDSN(t)
	p := startReplica(t, dsn, "127.0.0.1:0")
	other := startReplica(t, dsn, "127.0.0.1:0")
	p.withRepos()
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	svc, db := other.tasks()
	id := p.startTask("Fix the checkout")
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	if _, err := svc.Cancel(context.Background(), id); err != nil {
		t.Fatal(err)
	}
	eventually(t, "the runner removed the cancelled task", func() bool { return !m.host.holds(string(id)) })
}

func TestARelinkingRunnerKeepsWantedTasksAndStopsTheRest(t *testing.T) {
	dsn := pgtest.SchemaDSN(t)
	p := startReplica(t, dsn, "127.0.0.1:0")
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	id := p.startTask("Fix the checkout")
	eventually(t, "the task running", stateIs(t, db, id, domain.Running))
	addr := p.addr
	p.stop()
	ghost := wire.Fence{Task: "ghost", Run: 1, Gen: 0}
	if err := m.host.Up(context.Background(), runner.Box{Fence: ghost, BoxAllow: []string{"x"}}); err != nil {
		t.Fatal(err)
	}
	again := startReplica(t, dsn, addr)
	eventually(t, "the runner linked again", func() bool { return again.machines("Mia")[m.id].State == "linked" })
	eventually(t, "the unknown task stopped", func() bool { return !m.host.holds("ghost") })
	if !m.host.holds(string(id)) || m.host.launches(string(id)) != 1 {
		t.Fatalf("the re-adopted task: held %v, launched %d times", m.host.holds(string(id)), m.host.launches(string(id)))
	}
	if got, _ := db.Load(context.Background(), id); got.State != domain.Running || got.Run != 1 {
		t.Errorf("after the restart the task is %s at run %d", got.State, got.Run)
	}
}

func TestTwoReplicasOnOneDatabaseStartEachTaskOnce(t *testing.T) {
	dsn := pgtest.SchemaDSN(t)
	a := startReplica(t, dsn, "127.0.0.1:0")
	b := startReplica(t, dsn, "127.0.0.1:0")
	a.withRepos()
	_, db := a.tasks()
	ma, mb := enrol(t, a, "host-a"), enrol(t, b, "host-b")
	ma.run(t, a.addr)
	mb.run(t, b.addr)
	for round := range 6 {
		id := a.startTask("round")
		eventually(t, "the task running", stateIs(t, db, id, domain.Running))
		time.Sleep(150 * time.Millisecond)
		if n := ma.host.launches(string(id)) + mb.host.launches(string(id)); n != 1 {
			t.Fatalf("round %d: task %s launched %d times across the two machines", round, id, n)
		}
		close(ma.sessions.end(string(id)))
		close(mb.sessions.end(string(id)))
		eventually(t, "the task failed", stateIs(t, db, id, domain.Failed))
	}
}

func TestADrainedMachineTakesNoTaskAndARevokedOneFailsItsTasks(t *testing.T) {
	p := startReplica(t, pgtest.SchemaDSN(t), "127.0.0.1:0")
	p.withRepos()
	_, db := p.tasks()
	m := enrol(t, p, "host-a")
	m.run(t, p.addr)
	running := p.startTask("first")
	eventually(t, "the task running", stateIs(t, db, running, domain.Running))
	p.do(req{method: "POST", path: "/api/machines/" + m.id + "/drain", person: "Mia"}).has(t, 204)
	waiting := p.startTask("second")
	time.Sleep(300 * time.Millisecond)
	if got, _ := db.Load(context.Background(), waiting); got.State != domain.Waiting {
		t.Fatalf("a drained machine took a task: %s", got.State)
	}
	p.do(req{method: "POST", path: "/api/machines/" + m.id + "/revoke", person: "Mia"}).has(t, 204)
	if got, _ := db.Load(context.Background(), running); got.State != domain.Failed || got.Failure != "machine revoked" {
		t.Errorf("the task on the revoked machine is %s with %q", got.State, got.Failure)
	}
	select {
	case err := <-m.done:
		if !errors.Is(err, runner.ErrFatal) {
			t.Errorf("the revoked runner ended with %v, want a refusal", err)
		}
		m.done <- err
	case <-time.After(20 * time.Second):
		t.Fatal("the revoked runner kept running")
	}
}
