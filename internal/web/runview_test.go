package web

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

var start = time.Date(2026, 11, 28, 11, 42, 3, 0, time.UTC)

// script builds event rows at offsets from start, numbered from 1.
type script struct {
	id   domain.TaskID
	rows []domain.EventRow
}

func (s *script) add(after time.Duration, kind domain.EventKind, data any) *script {
	raw, err := json.Marshal(data)
	if err != nil {
		panic(err)
	}
	s.rows = append(s.rows, domain.EventRow{Task: s.id, Seq: int64(len(s.rows) + 1), At: start.Add(after), Kind: kind, Data: raw})
	return s
}

func (s *script) state(after time.Duration, from, to domain.State, reason string) *script {
	d := domain.StateData{To: to.String(), Reason: reason}
	if from != 0 {
		d.From = from.String()
	}
	return s.add(after, domain.KindState, d)
}

func (s *script) note(after time.Duration, phase, detail string) *script {
	return s.add(after, domain.KindNote, domain.NoteData{Text: domain.NoteText(phase, detail)})
}

func (s *script) publish(after time.Duration, version int, line string) *script {
	return s.add(after, domain.KindPublished, domain.PublishedData{Version: version, Line: line, EntryURL: "https://results.example/" + string(s.id) + "/v" + string(rune('0'+version))})
}

func sec(n int) time.Duration { return time.Duration(n) * time.Second }

func min2(m, s int) time.Duration { return time.Duration(m)*time.Minute + sec(s) }

// setUp plays the first 31 seconds of a task: queued, picked, desk and Box up, checked out, prompt sent, running.
func (s *script) setUp() *script {
	return s.state(0, 0, domain.Waiting, "started").
		state(sec(6), domain.Waiting, domain.Starting, "slot taken on build-1").
		note(sec(11), domain.PhaseDesk, "build-1").
		note(sec(22), domain.PhaseBox, "box:dev").
		note(sec(25), domain.PhaseSettings, "v4, 5 allow rules, 4 connections").
		note(sec(28), domain.PhaseCheckout, "main at 5f0e2b1").
		note(sec(31), domain.PhasePrompt, "").
		state(sec(31), domain.Starting, domain.Running, "desk, box and proxy up")
}

func taskIn(id domain.TaskID, state domain.State) domain.Task {
	return domain.Task{
		ID: id, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: "Add a coupon field\nto checkout", StartedBy: "Mia",
		State: state, Runner: "build-1", CreatedAt: start,
	}
}

func partOf(t *testing.T, v runView, key string) partView {
	t.Helper()
	p, ok := v.part(key)
	if !ok {
		t.Fatalf("no part %q in %+v", key, v.Parts)
	}
	return p
}

func keys(v runView) string {
	var out []string
	for _, p := range v.Parts {
		out = append(out, p.Key+":"+string(p.Mark))
	}
	return strings.Join(out, " ")
}

func TestARunningTaskShowsSetUpDoneAndAgentRunning(t *testing.T) {
	rows := (&script{id: "145"}).setUp().rows
	v := buildRun(taskIn("145", domain.Running), rows, start.Add(min2(3, 48)+sec(31)))
	if got := keys(v); got != "setup:done agent:running" {
		t.Fatalf("parts = %s", got)
	}
	setup, agent := partOf(t, v, "setup"), partOf(t, v, "agent")
	if setup.Head != "done in 31s" || setup.Dur != "31s" {
		t.Errorf("set up = %q %q", setup.Head, setup.Dur)
	}
	if agent.Head != "running for 3m 48s" || len(agent.Steps) != 0 {
		t.Errorf("agent = %q with %d steps", agent.Head, len(agent.Steps))
	}
	if !v.Actions.Cancel || !v.Actions.End || v.Actions.Rerun {
		t.Errorf("actions = %+v", v.Actions)
	}
	if !v.Waiting || v.Result != nil {
		t.Errorf("a running task with nothing published: waiting %v, result %v", v.Waiting, v.Result)
	}
}

func TestTheStartedByCardReadsTheTaskAndItsRows(t *testing.T) {
	rows := (&script{id: "145"}).setUp().rows
	v := buildRun(taskIn("145", domain.Running), rows, start.Add(min2(3, 48)+sec(31)))
	if v.Facts.Started != "Started by Mia 4m ago" || v.Facts.Detail != "from a prompt" || v.Facts.State != "Running" || v.Facts.Ref != "main" || v.Facts.Commit != "5f0e2b1" {
		t.Errorf("facts = %+v", v.Facts)
	}
	if v.Facts.Dur != "4m 19s" {
		t.Errorf("duration = %q", v.Facts.Dur)
	}
	if v.LastSeq != int64(len(rows)) {
		t.Errorf("last seq = %d", v.LastSeq)
	}
}

func TestSetUpStepsReadTheirLogsFromTheRows(t *testing.T) {
	rows := (&script{id: "145"}).setUp().rows
	steps := partOf(t, buildRun(taskIn("145", domain.Running), rows, start.Add(time.Minute)), "setup").Steps
	want := []struct {
		name, dur string
		log       []string
	}{
		{"Wait for a machine", "6s", []string{"queued at 11:42:03", "picked by build-1 at 11:42:09"}},
		{"Start desk and Box", "19s", []string{"desk up: build-1", "box up: box:dev", "settings applied: v4, 5 allow rules, 4 connections"}},
		{"Check out example-org/shop-web", "3s", []string{"main", "at 5f0e2b1"}},
		{"Send the prompt", "3s", []string{"typed by Mia", "", "Add a coupon field", "to checkout"}},
	}
	if len(steps) != len(want) {
		t.Fatalf("%d steps: %+v", len(steps), steps)
	}
	for i, w := range want {
		s := steps[i]
		if s.Name != w.name || s.Dur != w.dur || s.Mark != domain.GroupDone || strings.Join(s.Log, "|") != strings.Join(w.log, "|") {
			t.Errorf("step %d = %+v, want %+v", i, s, w)
		}
	}
}

func TestASpecTaskSendsItsPromptFromTheSpecFile(t *testing.T) {
	task := taskIn("142", domain.Running)
	task.Spec = &domain.Spec{Name: "PR evidence", File: "tasks/pr-evidence.yaml"}
	task.Event = &domain.Event{Source: "github", Group: "pr-90", Rev: "d3c1c85"}
	task.StartedBy, task.Prompt = domain.EventStarterPrefix+"github", "Show evidence."
	rows := (&script{id: "142"}).setUp().rows
	v := buildRun(task, rows, start.Add(time.Minute))
	if log := partOf(t, v, "setup").Steps[3].Log; log[0] != "from tasks/pr-evidence.yaml" {
		t.Errorf("prompt log = %v", log)
	}
	if v.Facts.Started != "Started by a github event 1m ago" || v.Facts.Detail != "PR evidence, pr-90" || v.Actions.End || v.HasSpec == false {
		t.Errorf("facts = %+v actions %+v", v.Facts, v.Actions)
	}
}

func TestAQueuedTaskIsSettingUpAndTheLaterStepsWait(t *testing.T) {
	rows := (&script{id: "144"}).state(0, 0, domain.Waiting, "started").rows
	v := buildRun(taskIn("144", domain.Waiting), rows, start.Add(sec(62)))
	if got := keys(v); got != "setup:running" {
		t.Fatalf("parts = %s", got)
	}
	var marks []domain.StateGroup
	for _, s := range partOf(t, v, "setup").Steps {
		marks = append(marks, s.Mark)
	}
	if len(marks) != 4 || marks[0] != domain.GroupRunning || marks[1] != markPending || marks[3] != markPending {
		t.Errorf("step marks = %v", marks)
	}
	if v.Waiting || !v.Actions.Cancel {
		t.Errorf("waiting %v, actions %+v", v.Waiting, v.Actions)
	}
}

func TestAFailureInStartingFailsSetUpAndInRunningFailsAgent(t *testing.T) {
	starting := (&script{id: "1"}).state(0, 0, domain.Waiting, "started").state(sec(4), domain.Waiting, domain.Starting, "slot").
		state(sec(20), domain.Starting, domain.Failed, "the Box image was not found")
	task1 := taskIn("1", domain.Failed)
	task1.Failure, task1.EndedAt = "the Box image was not found", start.Add(sec(20))
	v := buildRun(task1, starting.rows, start.Add(time.Hour))
	if got := keys(v); got != "setup:failed" {
		t.Errorf("a failed start has parts %s", got)
	}
	if v.Fail == nil || v.Fail.Part != "Set up" || v.Fail.Reason != "the Box image was not found" || !v.Actions.Rerun || v.Actions.Cancel {
		t.Errorf("fail %+v actions %+v", v.Fail, v.Actions)
	}
	running := (&script{id: "140"}).setUp().state(min2(2, 43)+sec(31), domain.Running, domain.Failed, "agent exited with code 1")
	task2 := taskIn("140", domain.Failed)
	task2.Failure, task2.EndedAt = "agent exited with code 1", start.Add(min2(3, 14))
	v = buildRun(task2, running.rows, start.Add(time.Hour))
	if got := keys(v); got != "setup:done agent:failed" {
		t.Errorf("a failed run has parts %s", got)
	}
	if agent := partOf(t, v, "agent"); agent.Head != "failed in 2m 43s" {
		t.Errorf("agent head = %q", agent.Head)
	}
	if v.Fail == nil || v.Fail.Part != "Agent" || v.Fail.Key != "agent" {
		t.Errorf("fail = %+v", v.Fail)
	}
	if v.Result != nil || v.Waiting {
		t.Error("a failed task with nothing published shows a result box")
	}
}

func TestACancelledTaskMarksThePartItWasIn(t *testing.T) {
	rows := (&script{id: "5"}).setUp().state(min2(1, 0), domain.Running, domain.Cancelled, "cancelled").rows
	task := taskIn("5", domain.Cancelled)
	task.EndedAt = start.Add(min2(1, 0))
	v := buildRun(task, rows, start.Add(time.Hour))
	if got := keys(v); got != "setup:done agent:cancelled" {
		t.Errorf("parts = %s", got)
	}
	if v.Actions != (actionsView{}) {
		t.Errorf("actions = %+v", v.Actions)
	}
}

// reviewed plays task 142: a review prompt, v1 before it, v2 after it, and an answer posted.
func reviewed() (domain.Task, []domain.EventRow) {
	rows := (&script{id: "142"}).setUp().
		publish(min2(6, 33), 1, "Five of five flows passed.").
		note(min2(6, 33), domain.PhaseReview, "").
		publish(min2(9, 34), 2, "Four of five flows passed.").
		state(min2(9, 34), domain.Running, domain.Delivering, "published version 2").
		add(min2(9, 36), domain.KindDelivered, domain.DeliveredData{Target: "pull request #90", ExternalID: "c-77", Version: 2}).
		state(min2(9, 36), domain.Delivering, domain.Done, "delivered version 2").rows
	t := taskIn("142", domain.Done)
	t.Spec = &domain.Spec{Name: "PR evidence", File: "tasks/pr-evidence.yaml"}
	t.Event = &domain.Event{Source: "github", Group: "pr-90", Rev: "d3c1c85"}
	t.StartedBy, t.ReviewPrompt, t.EndedAt = domain.EventStarterPrefix+"github", "Check your page.\nFix what fails.", start.Add(min2(9, 36))
	return t, rows
}

func TestAReviewedTaskHasAllFourPartsAndTheirTimes(t *testing.T) {
	task, rows := reviewed()
	v := buildRun(task, rows, start.Add(time.Hour))
	if got := keys(v); got != "setup:done agent:done review:done answer:done" {
		t.Fatalf("parts = %s", got)
	}
	for key, want := range map[string]string{"setup": "31s", "agent": "6m 02s", "review": "3m 01s", "answer": "2s"} {
		if got := partOf(t, v, key).Dur; got != want {
			t.Errorf("%s took %q, want %q", key, got, want)
		}
	}
	review := partOf(t, v, "review").Steps
	if len(review) != 2 || review[0].Name != "Send the review prompt" || review[1].Name != "Wait for the next publish" {
		t.Fatalf("review steps = %+v", review)
	}
	if got := strings.Join(review[0].Log, "|"); got != "from tasks/pr-evidence.yaml, after v1 at 11:48||Check your page.|Fix what fails." {
		t.Errorf("review prompt log = %q", got)
	}
	if got := strings.Join(review[1].Log, "|"); got != "published v2 at 11:51" {
		t.Errorf("next publish log = %q", got)
	}
	answer := partOf(t, v, "answer").Steps
	if len(answer) != 1 || answer[0].Name != "Comment on pull request #90" || answer[0].Mark != domain.GroupDone || answer[0].Dur != "2s" {
		t.Errorf("answer steps = %+v", answer)
	}
}

func TestTheSummaryShowsTheNewestVersionAndTheEarlierOnes(t *testing.T) {
	task, rows := reviewed()
	v := buildRun(task, rows, start.Add(time.Hour))
	if v.Result == nil || v.Result.Name != "v2" || v.Result.Line != "Four of five flows passed." || v.Result.At != "11:51" || v.Result.URL != "https://results.example/142/v2" {
		t.Errorf("result = %+v", v.Result)
	}
	if len(v.Earlier) != 1 || v.Earlier[0].Name != "v1" || v.Earlier[0].Note != "before the review" || v.Earlier[0].At != "11:48" {
		t.Errorf("earlier = %+v", v.Earlier)
	}
	if v.Answer == nil || v.Answer.Text != "Commented on pull request #90" || v.Answer.At != "posted 11:51" || v.Waiting {
		t.Errorf("answer = %+v waiting %v", v.Answer, v.Waiting)
	}
}

func TestAReopenAddsToTheAgentTime(t *testing.T) {
	rows := (&script{id: "9"}).setUp().publish(min2(2, 0), 1, "Done.").
		state(min2(2, 0), domain.Running, domain.Done, "published version 1").
		state(min2(5, 0), domain.Done, domain.Running, "teammate typed").rows
	task := taskIn("9", domain.Running)
	v := buildRun(task, rows, start.Add(min2(6, 0)))
	agent := partOf(t, v, "agent")
	if agent.Mark != domain.GroupRunning || agent.Dur != "2m 29s" {
		t.Errorf("agent after a reopen = %s %s, want running 2m 29s (1m 29s then 1m 00s)", agent.Mark, agent.Dur)
	}
	if v.Result == nil || v.Waiting {
		t.Errorf("a reopened task keeps its result: %+v waiting %v", v.Result, v.Waiting)
	}
}

func TestAResultPageIsLinkedOnlyWhenItsAddressIsSafe(t *testing.T) {
	for raw, want := range map[string]string{
		"https://results.example/a": "https://results.example/a", "http://127.0.0.1:9/a": "http://127.0.0.1:9/a", "/results/1/index.html": "/results/1/index.html",
		"javascript:alert(1)": "", "data:text/html,x": "", "file:///etc/passwd": "", "//evil.example/a": "", "": "", "https:///a": "", "ftp://x/a": "",
	} {
		if got := safeURL(raw); got != want {
			t.Errorf("safeURL(%q) = %q, want %q", raw, got, want)
		}
	}
}
