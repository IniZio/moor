package storetest

import (
	"context"
	"errors"
	"slices"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// MachineStore is the calls the machine logic makes, plus Save to put tasks on machines. It mirrors
// the interface in package machine, which this package cannot import.
type MachineStore interface {
	AddMachine(ctx context.Context, a domain.MachineAdd) error
	// JoinMachine returns domain.ErrJoinRefused for a token that is unknown, used, expired or for
	// another machine, and domain.ErrExists for a name another machine has.
	JoinMachine(ctx context.Context, j domain.MachineJoin) error
	// Machines lists every machine, oldest first.
	Machines(ctx context.Context) ([]domain.Machine, error)
	// Machine returns domain.ErrNotFound for no row.
	Machine(ctx context.Context, id domain.RunnerID) (domain.Machine, error)
	// Linked bumps the link generation and records the holder. It returns domain.ErrUnknownRunner
	// for a machine that has not joined or is revoked.
	Linked(ctx context.Context, id domain.RunnerID, build, holder string, at time.Time) (int64, error)
	// Heard records that the holder of link gen heard the runner, and reports false once gen is no
	// longer the newest or the machine is revoked.
	Heard(ctx context.Context, id domain.RunnerID, gen int64, at time.Time) (bool, error)
	// Unlinked clears the holder only while gen is still the newest.
	Unlinked(ctx context.Context, id domain.RunnerID, gen int64) error
	// DrainMachine keeps the time of the first drain.
	DrainMachine(ctx context.Context, id domain.RunnerID, at time.Time) error
	DisconnectMachine(ctx context.Context, id domain.RunnerID) error
	// ResumeMachine clears the drain, so the machine takes new tasks again. A machine that is not
	// draining is left as it is and is not an error.
	ResumeMachine(ctx context.Context, id domain.RunnerID) error
	RevokeMachine(ctx context.Context, id domain.RunnerID, at time.Time) error
	// RemoveMachine returns domain.ErrNotRevoked for a machine that is not revoked.
	RemoveMachine(ctx context.Context, id domain.RunnerID) error
	// WaitingTasks lists up to limit waiting tasks, oldest first.
	WaitingTasks(ctx context.Context, limit int) ([]domain.TaskID, error)
	// TasksOn lists the tasks on a machine that are starting, running or delivering, oldest first.
	TasksOn(ctx context.Context, id domain.RunnerID) ([]domain.Task, error)
	Save(ctx context.Context, w domain.TaskWrite) (domain.Task, error)
}

// RunMachines runs every machine case on a fresh store.
func RunMachines(t *testing.T, fresh func(t *testing.T) MachineStore) {
	t.Helper()
	cases := []struct {
		name string
		run  func(*testing.T, MachineStore)
	}{
		{"a new machine waits to join", newMachine},
		{"a join burns its token", joinBurns},
		{"a refused join changes nothing", joinRefused},
		{"two joins race for one token", joinRace},
		{"names are unique", uniqueNames},
		{"links are ordered by generation", linkOrder},
		{"only a joined, unrevoked machine links", linkRefused},
		{"drain, disconnect, revoke and remove", rowActions},
		{"resume puts a drained machine back in service", resumeMachine},
		{"tasks on machines", tasksOn},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) { c.run(t, fresh(t)) })
	}
}

func hash(s string) []byte { return []byte("hash-of-" + s) }

func add(t *testing.T, s MachineStore, id domain.RunnerID, token string) {
	t.Helper()
	a := domain.MachineAdd{ID: id, EnrolledBy: "ann", JoinHash: hash(token), JoinExpiresAt: at(3600), CreatedAt: at(1)}
	if err := s.AddMachine(context.Background(), a); err != nil {
		t.Fatalf("add %s: %v", id, err)
	}
}

func joinOf(id domain.RunnerID, token, name string) domain.MachineJoin {
	return domain.MachineJoin{ID: id, JoinHash: hash(token), Name: name, PubKey: []byte("key-" + name), At: at(60)}
}

func joined(t *testing.T, s MachineStore, id domain.RunnerID, name string) {
	t.Helper()
	add(t, s, id, "tok-"+string(id))
	if err := s.JoinMachine(context.Background(), joinOf(id, "tok-"+string(id), name)); err != nil {
		t.Fatalf("join %s: %v", id, err)
	}
}

func machine(t *testing.T, s MachineStore, id domain.RunnerID) domain.Machine {
	t.Helper()
	m, err := s.Machine(context.Background(), id)
	if err != nil {
		t.Fatalf("machine %s: %v", id, err)
	}
	return m
}

func newMachine(t *testing.T, s MachineStore) {
	add(t, s, "m1", "t1")
	m := machine(t, s, "m1")
	if m.ID != "m1" || m.EnrolledBy != "ann" || m.Name != "" || len(m.PubKey) != 0 || !m.CreatedAt.Equal(at(1)) || m.State(at(2)) != domain.MachineJoining {
		t.Errorf("new machine = %+v", m)
	}
	_, err := s.Machine(context.Background(), "nope")
	wantErr(t, "unknown machine", err, domain.ErrNotFound)
	all, err := s.Machines(context.Background())
	if err != nil || len(all) != 1 || all[0].ID != "m1" {
		t.Errorf("machines = %+v, %v", all, err)
	}
}

func joinBurns(t *testing.T, s MachineStore) {
	ctx := context.Background()
	add(t, s, "m1", "t1")
	if err := s.JoinMachine(ctx, joinOf("m1", "t1", "host-a")); err != nil {
		t.Fatal(err)
	}
	m := machine(t, s, "m1")
	if m.Name != "host-a" || string(m.PubKey) != "key-host-a" || m.State(at(61)) != domain.MachineOffline {
		t.Errorf("joined machine = %+v", m)
	}
	wantErr(t, "the same token again", s.JoinMachine(ctx, joinOf("m1", "t1", "host-b")), domain.ErrJoinRefused)
	if m := machine(t, s, "m1"); m.Name != "host-a" || string(m.PubKey) != "key-host-a" {
		t.Errorf("a second join changed the machine: %+v", m)
	}
}

func joinRefused(t *testing.T, s MachineStore) {
	ctx := context.Background()
	add(t, s, "m1", "t1")
	add(t, s, "m2", "t2")
	late := joinOf("m1", "t1", "host-a")
	late.At = at(3600)
	for what, j := range map[string]domain.MachineJoin{
		"an unknown token":        joinOf("m1", "nope", "host-a"),
		"another machine's token": joinOf("m1", "t2", "host-a"),
		"an unknown machine":      joinOf("m9", "t1", "host-a"),
		"a token at its expiry":   late,
	} {
		wantErr(t, what, s.JoinMachine(ctx, j), domain.ErrJoinRefused)
	}
	if err := s.RevokeMachine(ctx, "m2", at(5)); err != nil {
		t.Fatal(err)
	}
	wantErr(t, "a revoked machine", s.JoinMachine(ctx, joinOf("m2", "t2", "host-b")), domain.ErrJoinRefused)
	if err := s.JoinMachine(ctx, joinOf("m1", "t1", "host-a")); err != nil {
		t.Errorf("the right token after refusals: %v", err)
	}
}

func joinRace(t *testing.T, s MachineStore) {
	add(t, s, "m1", "t1")
	const racers = 8
	errs := make([]error, racers)
	start := make(chan struct{})
	done := make(chan int, racers)
	for i := range racers {
		go func() {
			<-start
			errs[i] = s.JoinMachine(context.Background(), joinOf("m1", "t1", "host-"+string(rune('a'+i))))
			done <- i
		}()
	}
	close(start)
	for range racers {
		<-done
	}
	won := 0
	for _, err := range errs {
		switch {
		case err == nil:
			won++
		case !errors.Is(err, domain.ErrJoinRefused):
			t.Errorf("racing join: %v", err)
		}
	}
	if won != 1 {
		t.Errorf("%d joins won one token, want 1", won)
	}
}

func uniqueNames(t *testing.T, s MachineStore) {
	ctx := context.Background()
	joined(t, s, "m1", "host-a")
	add(t, s, "m2", "t2")
	wantErr(t, "a taken name", s.JoinMachine(ctx, joinOf("m2", "t2", "host-a")), domain.ErrExists)
	if err := s.JoinMachine(ctx, joinOf("m2", "t2", "host-b")); err != nil {
		t.Errorf("the token after a taken name: %v", err)
	}
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func link(t *testing.T, s MachineStore, holder string, n int64) int64 {
	t.Helper()
	gen, err := s.Linked(context.Background(), "m1", "b1", holder, at(n))
	must(t, err)
	return gen
}

func wantHeard(t *testing.T, s MachineStore, gen int64, n int64, want bool) {
	t.Helper()
	if ok, err := s.Heard(context.Background(), "m1", gen, at(n)); ok != want || err != nil {
		t.Errorf("heard gen %d = %v, %v, want %v", gen, ok, err, want)
	}
}

func linkOrder(t *testing.T, s MachineStore) {
	joined(t, s, "m1", "host-a")
	g1 := link(t, s, "replica-1", 100)
	if m := machine(t, s, "m1"); m.Holder != "replica-1" || m.Build != "b1" || m.LinkGen != g1 || m.State(at(110)) != domain.MachineLinked || m.State(at(116)) != domain.MachineOffline {
		t.Errorf("linked machine = %+v", m)
	}
	g2 := link(t, s, "replica-2", 101)
	if g2 <= g1 {
		t.Fatalf("second link gen %d after %d", g2, g1)
	}
	wantHeard(t, s, g1, 102, false)
	wantHeard(t, s, g2, 120, true)
	if m := machine(t, s, "m1"); !m.HeardAt.Equal(at(120)) {
		t.Errorf("heard at %v, want %v", m.HeardAt, at(120))
	}
	must(t, s.Unlinked(context.Background(), "m1", g1))
	if m := machine(t, s, "m1"); m.Holder != "replica-2" {
		t.Errorf("the older link's end cleared the newer holder: %+v", m)
	}
	must(t, s.Unlinked(context.Background(), "m1", g2))
	if m := machine(t, s, "m1"); m.Holder != "" || m.State(at(121)) != domain.MachineOffline {
		t.Errorf("after the newest link ended: %+v", m)
	}
}

func linkRefused(t *testing.T, s MachineStore) {
	ctx := context.Background()
	add(t, s, "m1", "t1")
	_, err := s.Linked(ctx, "m1", "b1", "r", at(1))
	wantErr(t, "a machine that has not joined", err, domain.ErrUnknownRunner)
	_, err = s.Linked(ctx, "m9", "b1", "r", at(1))
	wantErr(t, "an unknown machine", err, domain.ErrUnknownRunner)
	joined(t, s, "m2", "host-b")
	if err := s.RevokeMachine(ctx, "m2", at(2)); err != nil {
		t.Fatal(err)
	}
	_, err = s.Linked(ctx, "m2", "b1", "r", at(3))
	wantErr(t, "a revoked machine", err, domain.ErrUnknownRunner)
}

func rowActions(t *testing.T, s MachineStore) {
	ctx := context.Background()
	joined(t, s, "m1", "host-a")
	gen := link(t, s, "r", 100)
	must(t, s.DrainMachine(ctx, "m1", at(101)))
	must(t, s.DrainMachine(ctx, "m1", at(105)))
	if m := machine(t, s, "m1"); !m.DrainingSince.Equal(at(101)) || m.State(at(106)) != domain.MachineDraining {
		t.Errorf("drained machine = %+v", m)
	}
	must(t, s.DisconnectMachine(ctx, "m1"))
	wantHeard(t, s, gen, 107, false)
	gen = link(t, s, "r", 108)
	wantErr(t, "remove before revoke", s.RemoveMachine(ctx, "m1"), domain.ErrNotRevoked)
	must(t, s.RevokeMachine(ctx, "m1", at(109)))
	wantHeard(t, s, gen, 110, false)
	if m := machine(t, s, "m1"); !m.RevokedAt.Equal(at(109)) || m.State(at(110)) != domain.MachineRevoked {
		t.Errorf("revoked machine = %+v", m)
	}
	must(t, s.RemoveMachine(ctx, "m1"))
	_, err := s.Machine(ctx, "m1")
	wantErr(t, "a removed machine", err, domain.ErrNotFound)
	wantErr(t, "removing an unknown machine", s.RemoveMachine(ctx, "m1"), domain.ErrNotFound)
}

func resumeMachine(t *testing.T, s MachineStore) {
	ctx := context.Background()
	joined(t, s, "m1", "host-a")
	link(t, s, "r", 100)
	must(t, s.DrainMachine(ctx, "m1", at(101)))
	must(t, s.ResumeMachine(ctx, "m1"))
	must(t, s.ResumeMachine(ctx, "m1"))
	if m := machine(t, s, "m1"); !m.DrainingSince.IsZero() || m.State(at(102)) != domain.MachineLinked {
		t.Errorf("resumed machine = %+v", m)
	}
	must(t, s.DrainMachine(ctx, "m1", at(110)))
	if m := machine(t, s, "m1"); !m.DrainingSince.Equal(at(110)) {
		t.Errorf("a drain after a resume must start a new drain, got %+v", m)
	}
	wantErr(t, "resuming an unknown machine", s.ResumeMachine(ctx, "m9"), domain.ErrNotFound)
}

func onMachine(id domain.TaskID, state domain.State, runner domain.RunnerID, n int64) domain.Task {
	t := person(id)
	t.State, t.Runner, t.CreatedAt = state, runner, at(n)
	return t
}

func tasksOn(t *testing.T, s MachineStore) {
	ctx := context.Background()
	joined(t, s, "m1", "host-a")
	joined(t, s, "m2", "host-b")
	for _, task := range []domain.Task{
		onMachine("w2", domain.Waiting, "", 3), onMachine("w1", domain.Waiting, "", 2), onMachine("w3", domain.Waiting, "", 4),
		onMachine("s1", domain.Starting, "m1", 5), onMachine("r1", domain.Running, "m1", 6), onMachine("d1", domain.Delivering, "m1", 7),
		onMachine("x1", domain.Done, "m1", 8), onMachine("c1", domain.Cancelled, "m1", 9), onMachine("r2", domain.Running, "m2", 10),
	} {
		if _, err := s.Save(ctx, domain.TaskWrite{Task: task, Create: true, Events: eventsFrom(task.ID, 1, 1)}); err != nil {
			t.Fatal(err)
		}
	}
	if got, err := s.WaitingTasks(ctx, 2); err != nil || !slices.Equal(got, []domain.TaskID{"w1", "w2"}) {
		t.Errorf("waiting = %v, %v", got, err)
	}
	on, err := s.TasksOn(ctx, "m1")
	if err != nil {
		t.Fatal(err)
	}
	var ids []domain.TaskID
	for _, task := range on {
		ids = append(ids, task.ID)
	}
	if !slices.Equal(ids, []domain.TaskID{"s1", "r1", "d1"}) {
		t.Errorf("tasks on m1 = %v", ids)
	}
	all, err := s.Machines(ctx)
	if err != nil || len(all) != 2 || all[0].Running != 3 || all[1].Running != 1 {
		t.Errorf("machines with their running counts = %+v, %v", all, err)
	}
}
