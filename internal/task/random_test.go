package task_test

import (
	"errors"
	"fmt"
	"math/rand/v2"
	"reflect"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

const (
	seedCount = 40
	stepCount = 300
)

// fuzz applies random operations to a few tasks and checks the invariants after every one.
type fuzz struct {
	*harness
	rng      *rand.Rand
	other    domain.Project
	previous map[domain.TaskID]domain.State
}

func (f *fuzz) pick(n int) int { return f.rng.IntN(n) }

func (f *fuzz) anyTask() (domain.TaskID, bool) {
	all := f.mem.Tasks()
	if len(all) == 0 {
		return "", false
	}
	return all[f.pick(len(all))].ID, true
}

// step runs one random operation and returns a function that repeats it exactly, or nil when the
// step is not a call to repeat.
func (f *fuzz) step() func() error {
	id, ok := f.anyTask()
	if !ok || f.pick(8) == 0 {
		return f.start()
	}
	switch n := f.pick(14); n {
	case 0:
		f.clock.Advance(time.Duration(f.pick(40)) * time.Minute)
		return nil
	case 1:
		return func() error { return errOf(f.svc.Retry(f.ctx(), domain.TaskID("retry-"+string(id)), id)) }
	default:
		return f.taskOp(id, n)
	}
}

func (f *fuzz) start() func() error {
	n := f.pick(10)
	project := f.proj
	if f.pick(3) == 0 {
		project = f.other
	}
	review := ""
	if f.pick(2) == 0 {
		review = "check it"
	}
	if n < 3 {
		in := task.StartInput{ID: domain.TaskID(fmt.Sprintf("p%d", f.pick(40))), Project: project, Prompt: "p", ReviewPrompt: review, By: "ann"}
		return func() error { return errOf(f.svc.StartByPerson(f.ctx(), in)) }
	}
	in := f.evIn(domain.TaskID(fmt.Sprintf("g%d", f.pick(60))), fmt.Sprintf("pr%d", f.pick(2)), fmt.Sprintf("rev%d", f.pick(4)))
	in.Project, in.ReviewPrompt = project, review
	if f.pick(3) == 0 {
		in.Event.Source = quiet
	}
	if f.pick(3) == 0 {
		in.Spec, in.CancelInProgress = nightly, false
	}
	return func() error { return errOf(f.svc.StartByEvent(f.ctx(), in)) }
}

func (f *fuzz) taskOp(id domain.TaskID, n int) func() error {
	t := f.get(id)
	run := t.Run
	if f.pick(10) == 0 {
		run--
	}
	call := func(r task.Report) func() error {
		r.Run = run
		return func() error { return errOf(f.svc.Report(f.ctx(), id, r)) }
	}
	digest := fmt.Sprintf("d%d", f.pick(3))
	switch n {
	case 2:
		return call(task.Report{Kind: task.SlotTaken, Runner: "r1"})
	case 3:
		return call(task.Report{Kind: task.PartsUp})
	case 4:
		return call(task.Report{Kind: task.PartFailed})
	case 5:
		return call(task.Report{Kind: task.HarnessExited})
	case 6:
		return call(task.Report{Kind: task.BoxRecreated, Gen: t.Gen + int64(f.pick(2))})
	case 7, 8:
		return func() error {
			return errOf(f.svc.Publish(f.ctx(), id, task.PublishInput{Line: "l", EntryURL: "u/" + digest, Digest: digest}))
		}
	case 9:
		v := 1 + f.pick(3)
		return func() error { return errOf(f.svc.Delivered(f.ctx(), id, task.DeliveredInput{Version: v})) }
	case 10:
		return func() error { return errOf(f.svc.End(f.ctx(), id)) }
	case 11:
		return func() error { return errOf(f.svc.Cancel(f.ctx(), id)) }
	case 12:
		return func() error { return errOf(f.svc.TimeLimit(f.ctx(), id)) }
	}
	return func() error { return errOf(f.svc.Steer(f.ctx(), id)) }
}

type world struct {
	tasks  []domain.Task
	events map[domain.TaskID]int
}

func (f *fuzz) world() world {
	w := world{tasks: f.mem.Tasks(), events: make(map[domain.TaskID]int)}
	for _, t := range w.tasks {
		w.events[t.ID] = len(f.mem.Events(t.ID))
	}
	return w
}

func TestRandomSequences(t *testing.T) {
	for seed := range uint64(seedCount) {
		t.Run(fmt.Sprint(seed), func(t *testing.T) { runSeed(t, seed) })
	}
}

func runSeed(t *testing.T, seed uint64) {
	f := &fuzz{harness: newHarness(t), rng: rand.New(rand.NewPCG(seed, 7)), previous: make(map[domain.TaskID]domain.State)}
	other, err := domain.NewProject("blog", f.proj.Entries())
	if err != nil {
		t.Fatal(err)
	}
	f.other = other
	for i := range stepCount {
		if call := f.step(); call != nil {
			f.exercise(t, i, call)
			f.invariants(t, i)
		}
	}
}

func (f *fuzz) exercise(t *testing.T, i int, call func() error) {
	t.Helper()
	err := call()
	if err != nil {
		if !refused(err) && !isOneOf(err, task.ErrStaleReport, task.ErrIDTaken, domain.ErrNotFound) {
			t.Fatalf("step %d: unexpected error %v", i, err)
		}
		return
	}
	before := f.world()
	if err := call(); err != nil {
		t.Fatalf("step %d: the repeat failed: %v", i, err)
	}
	if after := f.world(); !reflect.DeepEqual(before, after) {
		t.Fatalf("step %d: the repeat changed state or events", i)
	}
}

func (f *fuzz) invariants(t *testing.T, i int) {
	t.Helper()
	live := make(map[domain.TaskKey]int)
	for _, tk := range f.mem.Tasks() {
		f.checkTask(t, i, tk)
		if tk.Event != nil && *tk.Spec == spec && tk.State.Live() {
			live[domain.TaskKey{Project: tk.Project, Spec: tk.Spec.Name, Source: tk.Event.Source, Group: tk.Event.Group}]++
		}
	}
	for key, n := range live {
		if n > 1 {
			t.Fatalf("step %d: %d live tasks in %v", i, n, key)
		}
	}
}

func (f *fuzz) checkTask(t *testing.T, i int, tk domain.Task) {
	t.Helper()
	if _, err := domain.ParseState(tk.State.String()); err != nil {
		t.Fatalf("step %d: %s has state %v", i, tk.ID, tk.State)
	}
	if prev, seen := f.previous[tk.ID]; seen && !moved(tk.Flow(cfg.Answerable), prev, tk.State) {
		t.Fatalf("step %d: %s went %s to %s, which no event allows", i, tk.ID, prev, tk.State)
	}
	f.previous[tk.ID] = tk.State
	if (tk.State == domain.Delivering && !tk.Answers(cfg.Answerable)) || (tk.State == domain.Superseded && (tk.Spec == nil || *tk.Spec != spec)) {
		t.Fatalf("step %d: task %s with spec %+v and event %+v is %s", i, tk.ID, tk.Spec, tk.Event, tk.State)
	}
	f.checkRows(t, i, tk)
	f.checkVersions(t, i, tk)
}

func (f *fuzz) checkRows(t *testing.T, i int, tk domain.Task) {
	t.Helper()
	rows := f.mem.Events(tk.ID)
	for j, r := range rows {
		if r.Seq != int64(j)+1 {
			t.Fatalf("step %d: %s row %d has seq %d", i, tk.ID, j, r.Seq)
		}
	}
	if tk.LastSeq != int64(len(rows)) || tk.Version < 1 {
		t.Fatalf("step %d: %s last_seq %d for %d rows, version %d", i, tk.ID, tk.LastSeq, len(rows), tk.Version)
	}
}

func (f *fuzz) checkVersions(t *testing.T, i int, tk domain.Task) {
	t.Helper()
	versions, _ := f.mem.Versions(f.ctx(), tk.ID)
	digests := make(map[string]bool)
	for j, v := range versions {
		if v.N != j+1 || digests[v.Digest] {
			t.Fatalf("step %d: %s versions %+v", i, tk.ID, versions)
		}
		digests[v.Digest] = true
	}
	wantVersions := 1
	if tk.ReviewPrompt != "" {
		wantVersions = 2
	}
	if tk.State == domain.Delivering && len(versions) < wantVersions {
		t.Fatalf("step %d: %s delivers with %d versions, review prompt %q", i, tk.ID, len(versions), tk.ReviewPrompt)
	}
}

func moved(flow domain.Flow, from, to domain.State) bool {
	if from == to {
		return true
	}
	for ev := domain.SlotFree; ev <= domain.Cancel; ev++ {
		if next, err := domain.Next(flow, from, ev); err == nil && next == to {
			return true
		}
	}
	return false
}

func isOneOf(err error, targets ...error) bool {
	for _, target := range targets {
		if errors.Is(err, target) {
			return true
		}
	}
	return false
}
