package machine

import (
	"context"
	"io"
	"log/slog"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
	"github.com/IniZio/moor/internal/wire"
)

// window is the keep window, and limit a time limit no keep test runs into while its task runs.
const (
	window = 24 * time.Hour
	limit  = 3 * window
)

// readStore is the machine's Store as far as reconcile reads it: the tasks in memory, none placed.
type readStore struct {
	Store
	mem *tasktest.Memory
}

func (s readStore) Load(ctx context.Context, id domain.TaskID) (domain.Task, error) {
	return s.mem.Load(ctx, id) //nolint:wrapcheck // a test double
}

func (readStore) TasksOn(context.Context, domain.RunnerID) ([]domain.Task, error) { return nil, nil }

// fakeLink is the runner's end of a link: it lists the tasks it holds and records the ops sent to it.
type fakeLink struct {
	mu   sync.Mutex
	held []string
	ops  []wire.Kind
	rels []string
}

func (l *fakeLink) Open(_ context.Context, op wire.Kind, body, out any) (Stream, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.ops = append(l.ops, op)
	if op == wire.KindTaskList {
		reply := out.(*wire.TaskListReply)
		for _, id := range l.held {
			reply.Tasks = append(reply.Tasks, wire.ListedTask{Fence: wire.Fence{Task: id, Run: 1}})
		}
	}
	if rel, ok := body.(wire.TaskRelease); ok {
		l.rels = append(l.rels, rel.Task)
	}
	return nopStream{}, nil
}

func (*fakeLink) Done() <-chan struct{} { return nil }
func (*fakeLink) Close() error          { return nil }

func (l *fakeLink) count(op wire.Kind) int {
	l.mu.Lock()
	defer l.mu.Unlock()
	n := 0
	for _, o := range l.ops {
		if o == op {
			n++
		}
	}
	return n
}

type nopStream struct{}

func (nopStream) Recv(any) error              { return io.EOF }
func (nopStream) Read([]byte) (int, error)    { return 0, io.EOF }
func (nopStream) Write(p []byte) (int, error) { return len(p), nil }
func (nopStream) Close() error                { return nil }

// keepRig is a portal replica's view of one machine that holds one task.
type keepRig struct {
	t     *testing.T
	mem   *tasktest.Memory
	clock *tasktest.Clock
	svc   *task.Service
	link  *fakeLink
	h     *holding
}

func newKeepRig(t *testing.T) *keepRig {
	t.Helper()
	mem := tasktest.NewMemory()
	clock := tasktest.NewClock(time.Date(2026, 11, 28, 9, 0, 0, 0, time.UTC))
	svc := task.New(mem, clock, task.Config{TimeLimit: limit, KeepWindow: window})
	l := &fakeLink{}
	c := NewControl(ControlConfig{Store: readStore{mem: mem}, Tasks: svc, Clock: clock, Log: slog.New(slog.DiscardHandler)})
	return &keepRig{t: t, mem: mem, clock: clock, svc: svc, link: l, h: &holding{c: c, id: "r1", link: l, log: c.cfg.Log, starting: map[domain.TaskID]bool{}}}
}

// taskOn starts a person's task, runs it on r1 and leaves the runner holding its containers.
func (k *keepRig) taskOn(id domain.TaskID) {
	k.t.Helper()
	ctx := context.Background()
	if _, err := k.svc.StartByPerson(ctx, task.StartInput{ID: id, Repo: domain.Repo{Host: "forge.example", Path: "acme/app"}, Prompt: "p", By: "ann"}); err != nil {
		k.t.Fatal(err)
	}
	for _, r := range []task.Report{{Kind: task.SlotTaken, Runner: "r1"}, {Kind: task.PartsUp, Run: 1, Runner: "r1"}} {
		if _, err := k.svc.Report(ctx, id, r); err != nil {
			k.t.Fatal(err)
		}
	}
	k.link.held = append(k.link.held, string(id))
}

func (k *keepRig) reconcile() {
	k.t.Helper()
	if err := k.h.reconcile(context.Background()); err != nil {
		k.t.Fatal(err)
	}
}

func (k *keepRig) rows(id domain.TaskID) int {
	n := 0
	for _, r := range k.mem.Events(id) {
		if r.Kind == domain.KindNote && string(r.Data) == `{"text":"containers released: keep window ended"}` {
			n++
		}
	}
	return n
}

func (k *keepRig) end(id domain.TaskID) {
	k.t.Helper()
	if _, err := k.svc.End(context.Background(), id); err != nil {
		k.t.Fatal(err)
	}
}

func TestADoneTaskKeepsItsContainersForTheWindowThenTheyAreReleasedOnceWithAnEvent(t *testing.T) {
	k := newKeepRig(t)
	k.taskOn("t1")
	k.end("t1")
	k.clock.Advance(window - time.Second)
	k.reconcile()
	if len(k.link.rels) != 0 || k.rows("t1") != 0 {
		t.Fatalf("inside the window: released %v, %d release rows", k.link.rels, k.rows("t1"))
	}
	k.clock.Advance(time.Second)
	k.reconcile()
	k.reconcile()
	if len(k.link.rels) != 2 || k.link.rels[0] != "t1" || k.rows("t1") != 1 {
		t.Fatalf("after the window: released %v (the runner still lists it, so the release repeats), %d release rows, want one", k.link.rels, k.rows("t1"))
	}
	if k.link.count(wire.KindTaskStop) != 0 {
		t.Error("a task past its window was stopped, not released")
	}
}

func TestAReopenInsideTheWindowRestartsTheClockForTheReleaseToo(t *testing.T) {
	k := newKeepRig(t)
	k.taskOn("t1")
	k.end("t1")
	k.clock.Advance(window / 2)
	if _, err := k.svc.Steer(context.Background(), "t1"); err != nil {
		t.Fatal(err)
	}
	k.clock.Advance(window)
	k.reconcile()
	if len(k.link.rels) != 0 {
		t.Fatalf("a reopened, running task was released: %v", k.link.rels)
	}
	k.end("t1")
	k.clock.Advance(window - time.Second)
	k.reconcile()
	if len(k.link.rels) != 0 {
		t.Fatalf("the new window was cut short: %v", k.link.rels)
	}
	k.clock.Advance(time.Second)
	k.reconcile()
	if len(k.link.rels) != 1 || k.rows("t1") != 1 {
		t.Fatalf("released %v with %d rows after the new window", k.link.rels, k.rows("t1"))
	}
}

func TestAFailedTaskIsKeptForTheWindowToo(t *testing.T) {
	k := newKeepRig(t)
	k.taskOn("t1")
	if _, err := k.svc.MachineRevoked(context.Background(), "t1"); err != nil {
		t.Fatal(err)
	}
	k.clock.Advance(window - time.Second)
	k.reconcile()
	k.clock.Advance(time.Second)
	k.reconcile()
	if len(k.link.rels) != 1 || k.rows("t1") != 1 {
		t.Fatalf("released %v with %d rows", k.link.rels, k.rows("t1"))
	}
}

func TestARunningTaskFailsAtItsTimeLimitAndKeepsItsContainersForTheWindow(t *testing.T) {
	k := newKeepRig(t)
	k.taskOn("t1")
	k.clock.Advance(limit - time.Second)
	k.reconcile()
	if got, _ := k.mem.Load(context.Background(), "t1"); got.State != domain.Running {
		t.Fatalf("before the limit the task is %s", got.State)
	}
	k.clock.Advance(time.Second)
	k.reconcile()
	got, _ := k.mem.Load(context.Background(), "t1")
	if got.State != domain.Failed || got.Failure != "time limit reached" {
		t.Fatalf("at the limit the task is %s with %q", got.State, got.Failure)
	}
	if k.link.count(wire.KindTaskStop) != 0 || len(k.link.rels) != 0 {
		t.Fatal("a task that failed at its limit lost its containers inside the keep window")
	}
	k.clock.Advance(window)
	k.reconcile()
	if len(k.link.rels) != 1 {
		t.Fatalf("released %v after the window", k.link.rels)
	}
}

func TestACancelledTaskAndATaskThePortalDoesNotKnowAreStoppedAtOnce(t *testing.T) {
	k := newKeepRig(t)
	k.taskOn("t1")
	if _, err := k.svc.Cancel(context.Background(), "t1"); err != nil {
		t.Fatal(err)
	}
	k.link.held = append(k.link.held, "stray")
	k.reconcile()
	if k.link.count(wire.KindTaskStop) != 2 || len(k.link.rels) != 0 {
		t.Fatalf("stops %d, releases %v, want both tasks stopped", k.link.count(wire.KindTaskStop), k.link.rels)
	}
}
