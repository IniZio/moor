package answer_test

import (
	"context"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/answer"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/task/tasktest"
)

type post struct{ id, body string }

// poster keeps its comments in a list, like a forge. Lost makes Post's id unknown to the caller.
type poster struct {
	comments []post
	creates  int
	edits    int
	failNext error
}

func (p *poster) Target(ev domain.Event) (domain.DeliveryTarget, bool) {
	return domain.DeliveryTarget{Kind: "comment", Ref: string(ev.Group), Label: "pull request #7"}, true
}

func (p *poster) Body(d domain.Delivery) string {
	out := "answer " + d.Ref
	for spec, s := range d.Sections {
		out += "|" + string(spec) + ":" + s.Line
	}
	return out
}

func (p *poster) Find(_ context.Context, _ domain.DeliveryTarget) (string, bool, error) {
	if len(p.comments) > 0 {
		return p.comments[0].id, true, nil
	}
	return "", false, nil
}

func (p *poster) Post(_ context.Context, _ domain.DeliveryTarget, body string) (string, error) {
	if err := p.fail(); err != nil {
		return "", err
	}
	p.creates++
	p.comments = append(p.comments, post{id: "c" + strconv.Itoa(len(p.comments)+1), body: body})
	return p.comments[len(p.comments)-1].id, nil
}

func (p *poster) Edit(_ context.Context, _ domain.DeliveryTarget, id, body string) error {
	if err := p.fail(); err != nil {
		return err
	}
	p.edits++
	for i := range p.comments {
		if p.comments[i].id == id {
			p.comments[i].body = body
		}
	}
	return nil
}

func (p *poster) fail() error {
	err := p.failNext
	p.failNext = nil
	return err
}

// deliveries keeps delivery rows in memory and lets one caller at a time in.
type deliveries struct {
	mu   sync.Mutex
	rows map[string]domain.Delivery
	lose bool
}

func (d *deliveries) Locked(_ context.Context, kind, ref string, fn func(domain.Delivery) (domain.Delivery, error)) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	cur := d.rows[kind+ref]
	cur.Kind, cur.Ref = kind, ref
	out, err := fn(cur)
	if err != nil {
		return err
	}
	if d.lose {
		out.ExternalID = ""
	}
	d.rows[kind+ref] = out
	return nil
}

type rig struct {
	t      *testing.T
	svc    *answer.Service
	tasks  *task.Service
	mem    *tasktest.Memory
	poster *poster
	rows   *deliveries
	n      int
}

func newRig(t *testing.T) *rig {
	t.Helper()
	mem := tasktest.NewMemory()
	clock := tasktest.NewClock(time.Date(2026, 11, 28, 9, 0, 0, 0, time.UTC))
	tasks := task.New(mem, clock, task.Config{Answerable: map[domain.EventSource]bool{"forge": true}})
	p, rows := &poster{}, &deliveries{rows: map[string]domain.Delivery{}}
	svc := answer.New(answer.Deps{Tasks: tasks, Reads: mem, Deliveries: rows, Posters: map[domain.EventSource]answer.Poster{"forge": p}, Clock: clock})
	return &rig{t: t, svc: svc, tasks: tasks, mem: mem, poster: p, rows: rows}
}

// running starts a task of spec for the pull request group and takes it to running.
func (r *rig) running(spec string) domain.TaskID {
	r.t.Helper()
	r.n++
	id := domain.TaskID("t" + strconv.Itoa(r.n))
	ctx := context.Background()
	_, err := r.tasks.StartByEvent(ctx, task.EventInput{
		ID: id, Repo: domain.Repo{Host: "forge.example", Path: "acme/app"}, Spec: domain.Spec{Name: domain.SpecName(spec), File: "tasks/" + spec + ".yaml"},
		Event: domain.Event{Source: "forge", Group: "acme/app#7", Rev: "abc"}, Prompt: "p",
	})
	if err != nil {
		r.t.Fatal(err)
	}
	for _, rep := range []task.Report{{Kind: task.SlotTaken, Runner: "r1"}, {Kind: task.PartsUp, Run: 1}} {
		if _, err := r.tasks.Report(ctx, id, rep); err != nil {
			r.t.Fatal(err)
		}
	}
	return id
}

func (r *rig) publish(id domain.TaskID, digest, line string) error {
	_, err := r.svc.Publish(context.Background(), id, task.PublishInput{Line: line, EntryURL: "https://results.example/" + digest, Digest: digest})
	if err != nil {
		return fmt.Errorf("publish: %w", err)
	}
	return nil
}

func (r *rig) state(id domain.TaskID) domain.State {
	r.t.Helper()
	got, err := r.mem.Load(context.Background(), id)
	if err != nil {
		r.t.Fatal(err)
	}
	return got.State
}

func TestTwoSpecsOnOnePullRequestShareOneCommentWithOneSectionEach(t *testing.T) {
	r := newRig(t)
	a, b := r.running("evidence"), r.running("review")
	if err := r.publish(a, "d1", "all passed"); err != nil {
		t.Fatal(err)
	}
	if err := r.publish(b, "d2", "looks fine"); err != nil {
		t.Fatal(err)
	}
	if r.poster.creates != 1 || r.poster.edits != 1 || len(r.poster.comments) != 1 {
		t.Fatalf("%d creates, %d edits, %d comments", r.poster.creates, r.poster.edits, len(r.poster.comments))
	}
	body := r.poster.comments[0].body
	for _, want := range []string{"evidence:all passed", "review:looks fine"} {
		if !strings.Contains(body, want) {
			t.Errorf("the comment %q lacks %q", body, want)
		}
	}
	if r.state(a) != domain.Done || r.state(b) != domain.Done {
		t.Errorf("states %s and %s", r.state(a), r.state(b))
	}
}

func TestAnAnswerWhoseIdTheStoreLostIsFoundAgainAndNotPostedTwice(t *testing.T) {
	r := newRig(t)
	r.rows.lose = true
	a, b := r.running("evidence"), r.running("review")
	if err := r.publish(a, "d1", "all passed"); err != nil {
		t.Fatal(err)
	}
	if err := r.publish(b, "d2", "looks fine"); err != nil {
		t.Fatal(err)
	}
	if r.poster.creates != 1 || len(r.poster.comments) != 1 {
		t.Fatalf("%d creates, %d comments", r.poster.creates, len(r.poster.comments))
	}
}

func TestAFailedPostIsRecordedAndNotTriedAgainByItself(t *testing.T) {
	r := newRig(t)
	id := r.running("evidence")
	r.poster.failNext = errors.New("502 Bad Gateway")
	if err := r.publish(id, "d1", "all passed"); err == nil {
		t.Fatal("no error")
	}
	if r.state(id) != domain.Delivering || r.poster.creates != 0 {
		t.Fatalf("state %s, %d creates", r.state(id), r.poster.creates)
	}
	if err := r.publish(id, "d1", "all passed"); err != nil {
		t.Fatal(err)
	}
	if r.state(id) != domain.Done || r.poster.creates != 1 {
		t.Errorf("state %s, %d creates", r.state(id), r.poster.creates)
	}
}

func TestAPlainTaskPostsNothing(t *testing.T) {
	r := newRig(t)
	ctx := context.Background()
	if _, err := r.tasks.StartByPerson(ctx, task.StartInput{ID: "p1", Repo: domain.Repo{Host: "forge.example", Path: "acme/app"}, Prompt: "x", By: "Mia"}); err != nil {
		t.Fatal(err)
	}
	for _, rep := range []task.Report{{Kind: task.SlotTaken, Runner: "r1"}, {Kind: task.PartsUp, Run: 1}} {
		if _, err := r.tasks.Report(ctx, "p1", rep); err != nil {
			t.Fatal(err)
		}
	}
	if err := r.publish("p1", "d1", "done"); err != nil {
		t.Fatal(err)
	}
	if r.poster.creates != 0 || r.state("p1") != domain.Done {
		t.Errorf("%d creates, state %s", r.poster.creates, r.state("p1"))
	}
}
