package main

import (
	"context"
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

const auditSpec = `name: Dependency audit
repositories:
  - example-org/shop-web
on:
  schedule:
    - cron: "0 6 * * 1"
prompt: Audit ${{ event.repository.full_name }} on ${{ event.repository.default_branch }} for ${{ event.schedule }}.
`

const sweepSpec = `name: CVE sweep
repositories:
  - example-org/shop-web
on:
  schedule:
    - cron: "0 6 * * 1"
  workflow_dispatch:
concurrency:
  cancel-in-progress: true
prompt: Sweep ${{ event.repository.html_url }} at ${{ event.repository.default_branch }}.
`

// monday is a minute the audit and the sweep fire in, with the replica waking 30 seconds into it.
var monday = time.Date(2026, 11, 30, 6, 0, 30, 0, time.UTC)

func (r *hookRig) tick(at time.Time) {
	r.t.Helper()
	if err := r.portal.events.Tick(context.Background(), at); err != nil {
		r.t.Fatal(err)
	}
}

func (r *hookRig) taskIDs() []string {
	r.t.Helper()
	var ids []string
	for _, tk := range r.list("Mia", "").Tasks {
		ids = append(ids, tk.ID)
	}
	return ids
}

func TestAScheduleStartsOneTaskOnItsRepositoryInTheMinuteItNames(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/audit.yaml": auditSpec})
	r.fake.SetDefaultBranch("example-org/shop-web", "trunk")
	r.tick(monday.Add(-time.Minute))
	r.tick(monday)
	r.tick(monday.Add(time.Minute))
	r.tick(monday.Add(24 * time.Hour))
	ids := r.taskIDs()
	if len(ids) != 1 {
		t.Fatalf("tasks %v, want one", ids)
	}
	got := r.load(ids[0])
	if got.Prompt != "Audit example-org/shop-web on trunk for 0 6 * * 1." || got.Repo != shopWeb || got.StartedBy != "event:portal" || got.State != domain.Waiting {
		t.Errorf("task %+v", got)
	}
	if got.Event.Group != "tasks/audit.yaml@github.com/example-org/shop-web" || got.Event.Rev != "2026-11-30T06:00 UTC" || got.Ref != "" || got.Commit != "" {
		t.Errorf("event %+v, ref %q, commit %q", got.Event, got.Ref, got.Commit)
	}
	if !strings.Contains(r.do(get("Mia", "/tasks")).body, "#"+ids[0]+": schedule, Monday 06:00") {
		t.Errorf("the Tasks page does not say a schedule started it")
	}
}

func TestTwoReplicasTickingTheSameMinuteStartOneTask(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/audit.yaml": auditSpec})
	other, err := assemble(context.Background(), r.cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(other.close)
	var wg sync.WaitGroup
	errs := make([]error, 2)
	for i, p := range []*portal{r.portal, other} {
		wg.Go(func() { errs[i] = p.events.Tick(context.Background(), monday.Add(time.Duration(i)*time.Second)) })
	}
	wg.Wait()
	if errs[0] != nil || errs[1] != nil {
		t.Fatalf("ticks: %v", errs)
	}
	if ids := r.taskIDs(); len(ids) != 1 {
		t.Errorf("tasks %v, want one", ids)
	}
}

// fakeTimer is a clock that jumps forward by what each wait asks plus late, and ends the loop after waits waits.
type fakeTimer struct {
	now   time.Time
	late  time.Duration
	waits int
}

func (f *fakeTimer) Now() time.Time { return f.now }

func (f *fakeTimer) Wait(_ context.Context, d time.Duration) bool {
	if f.waits == 0 {
		return false
	}
	f.waits--
	f.now = f.now.Add(d + f.late)
	return true
}

func TestTheClockFiresAMinuteItWakesInEvenLateAndNeverAMinuteItSlept(t *testing.T) {
	cases := map[string]struct {
		late time.Duration
		want int
	}{
		"on the minute":     {0, 1},
		"59 seconds late":   {59 * time.Second, 1},
		"asleep through it": {61 * time.Second, 0},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			r := newHookRig(t, map[string]string{"tasks/audit.yaml": auditSpec})
			clock := &fakeTimer{now: time.Date(2026, 11, 30, 5, 59, 20, 0, time.UTC), late: c.late, waits: 1}
			r.portal.events.Loop(context.Background(), clock)
			if ids := r.taskIDs(); len(ids) != c.want {
				t.Errorf("tasks %v, want %d", ids, c.want)
			}
		})
	}
}

func TestAScheduleInATimeZoneFiresAMinuteTheClockRepeatsOnce(t *testing.T) {
	spec := strings.Replace(auditSpec, `- cron: "0 6 * * 1"`, "- cron: \"30 1 * * *\"\n      timezone: America/New_York", 1)
	r := newHookRig(t, map[string]string{"tasks/audit.yaml": spec})
	for h := range 8 {
		r.tick(time.Date(2026, 11, 1, h, 30, 0, 0, time.UTC))
	}
	ids := r.taskIDs()
	if len(ids) != 1 {
		t.Fatalf("tasks %v, want one for 01:30 that New York has twice", ids)
	}
	if got := r.load(ids[0]).Event.Rev; got != "2026-11-01T01:30 America/New_York" {
		t.Errorf("rev %q", got)
	}
}

func TestAScheduleWithoutRepositoriesFiresNothingAndItsSpecSaysSo(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/audit.yaml": strings.Replace(auditSpec, "repositories:\n  - example-org/shop-web\n", "", 1)})
	r.tick(monday)
	if ids := r.taskIDs(); len(ids) != 0 {
		t.Errorf("tasks %v", ids)
	}
	r.do(get("Mia", "/api/task-specs")).has(t, http.StatusOK, "The schedule starts no task: list the repositories it runs on under repositories in the spec.")
}

func (r *hookRig) runOn(spec, input string) reply {
	b, _ := json.Marshal(map[string]string{"input": input})
	return r.do(req{method: http.MethodPost, path: "/api/task-specs/" + url.PathEscape(spec) + "/run", person: "Ken", json: true, body: string(b)})
}

func TestRunOnARepositoryStartsTheTaskTheScheduleWouldWithTheDefaultBranch(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/sweep.yaml": sweepSpec})
	r.fake.SetDefaultBranch("example-org/shop-web", "trunk")
	page := r.do(get("Mia", "/tasks?spec="+url.QueryEscape("CVE sweep")))
	page.has(t, http.StatusOK, "Repository", `<select id="runinput" name="input" required>`, `<option value="github.com/example-org/shop-web">example-org/shop-web</option>`)
	res := r.runOn("CVE sweep", "example-org/shop-web")
	res.has(t, http.StatusOK)
	var out struct{ Task struct{ ID string } }
	if err := json.Unmarshal([]byte(res.body), &out); err != nil {
		t.Fatal(err)
	}
	got := r.load(out.Task.ID)
	if got.Prompt != "Sweep https://github.com/example-org/shop-web at trunk." || got.StartedBy != "Ken" || got.TokenOwner != "Ken" || got.Ref != "" {
		t.Errorf("task %+v", got)
	}
	if got.Event.Source != "portal" || got.Event.Group != "tasks/sweep.yaml@github.com/example-org/shop-web" || got.Event.Rev != domain.Revision("run-"+out.Task.ID) {
		t.Errorf("event %+v", got.Event)
	}
	form := hx(req{method: http.MethodPost, path: "/api/task-specs/CVE%20sweep/run", person: "Ken", form: url.Values{"input": {"github.com/example-org/shop-web"}}})
	r.do(form).has(t, http.StatusOK, "Started task #2 on example-org/shop-web.")
	if !strings.Contains(r.do(get("Mia", "/tasks")).body, "#2: Run by Ken") {
		t.Errorf("the Tasks page does not say who ran it")
	}
}

func TestRunOnARepositoryRefusesOneTheSpecOrSettingsLeaveOut(t *testing.T) {
	open := strings.Replace(sweepSpec, "repositories:\n  - example-org/shop-web\n", "", 1)
	r := newHookRig(t, map[string]string{"tasks/sweep.yaml": sweepSpec, "tasks/open.yaml": strings.Replace(open, "CVE sweep", "Open sweep", 1)})
	r.runOn("CVE sweep", "example-org/other").has(t, http.StatusUnprocessableEntity, "example-org/other is not under repositories in Settings.")
	r.runOn("CVE sweep", "not a repository").has(t, http.StatusUnprocessableEntity, "Pick a repository, such as example-org/shop-web.")
	r.runOn("Open sweep", "example-org/other").has(t, http.StatusUnprocessableEntity, "is not under repositories in Settings")
	if ids := r.taskIDs(); len(ids) != 0 {
		t.Errorf("tasks %v", ids)
	}
	r.do(get("Mia", "/tasks?spec="+url.QueryEscape("Open sweep"))).has(t, http.StatusOK, `<option value="github.com/example-org/shop-web">`)
	r.runOn("Open sweep", "example-org/shop-web").has(t, http.StatusOK)
}

func TestARunOnARepositoryCancelsTheScheduledTaskOfASpecThatCancelsInProgress(t *testing.T) {
	r := newHookRig(t, map[string]string{"tasks/sweep.yaml": sweepSpec})
	r.tick(monday)
	first := r.taskIDs()[0]
	r.running(first)
	r.runOn("CVE sweep", "example-org/shop-web").has(t, http.StatusOK)
	if got := r.load(first); got.State != domain.Superseded {
		t.Errorf("the scheduled task is %s", got.State)
	}
}
