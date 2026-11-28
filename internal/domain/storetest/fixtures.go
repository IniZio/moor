package storetest

import (
	"context"
	"errors"
	"reflect"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

func at(n int64) time.Time { return time.Unix(1_700_000_000+n, 123456000).UTC() }

// triggered is a waiting task a trigger started, with one event's worth of last_seq.
func triggered(id domain.TaskID, rev domain.Revision) domain.Task {
	tr := &domain.Trigger{Name: "review", Source: "forge", Group: "g", Rev: rev, Context: []byte(`{"n":1}`)}
	return domain.Task{ID: id, Project: "shop", Prompt: "p", Trigger: tr, State: domain.Waiting, LastSeq: 1, CreatedAt: at(1)}
}

func person(id domain.TaskID) domain.Task {
	return domain.Task{ID: id, Project: "shop", Prompt: "p", State: domain.Waiting, LastSeq: 1, CreatedAt: at(1)}
}

func full(id domain.TaskID) domain.Task {
	t := triggered(id, "r1")
	t.StartedBy, t.TokenOwner = "ann", "bob"
	t.ReviewPrompt, t.Reply = "check the page", domain.ReplyTarget{Source: "forge", Ref: "acme/app#7"}
	t.State, t.Runner, t.Run, t.Gen, t.LastSeq = domain.Running, "m1", 2, 3, 2
	t.Pins = map[domain.Dir]domain.Commit{"api": "abc", "web": "def"}
	t.Refs = map[domain.Dir]string{"api": "main"}
	t.Deadline, t.KeepUntil, t.EndedAt, t.Failure = at(10), at(20), at(30), "boom"
	return t
}

func event(id domain.TaskID, seq int64) domain.EventRow {
	return domain.EventRow{Task: id, Seq: seq, At: at(seq), Kind: domain.KindNote, Data: []byte(`{"text":"x"}`)}
}

func eventsFrom(id domain.TaskID, from, to int64) []domain.EventRow {
	var rows []domain.EventRow
	for seq := from; seq <= to; seq++ {
		rows = append(rows, event(id, seq))
	}
	return rows
}

// create stores task as new with events 1 to its LastSeq and returns it as stored.
func create(t *testing.T, s Subject, task domain.Task) domain.Task {
	t.Helper()
	saved, err := s.Store.Save(context.Background(), domain.TaskWrite{Task: task, Create: true, Events: eventsFrom(task.ID, 1, task.LastSeq)})
	if err != nil {
		t.Fatalf("create %s: %v", task.ID, err)
	}
	return saved
}

func load(t *testing.T, s Subject, id domain.TaskID) domain.Task {
	t.Helper()
	got, err := s.Store.Load(context.Background(), id)
	if err != nil {
		t.Fatalf("load %s: %v", id, err)
	}
	return got
}

func events(t *testing.T, s Subject, id domain.TaskID) []domain.EventRow {
	t.Helper()
	rows, err := s.Events(context.Background(), id)
	if err != nil {
		t.Fatalf("events %s: %v", id, err)
	}
	return rows
}

func wantErr(t *testing.T, what string, err, want error) {
	t.Helper()
	if !errors.Is(err, want) {
		t.Errorf("%s: err = %v, want %v", what, err, want)
	}
}

// sameTask compares times by instant and treats an empty map as a missing one, which is all a
// database can keep apart.
func sameTask(t *testing.T, what string, got, want domain.Task) {
	t.Helper()
	pairs := [][2]time.Time{
		{got.Deadline, want.Deadline},
		{got.KeepUntil, want.KeepUntil},
		{got.CreatedAt, want.CreatedAt},
		{got.EndedAt, want.EndedAt},
	}
	for i, p := range pairs {
		if !p[0].Equal(p[1]) {
			t.Errorf("%s: time %d = %v, want %v", what, i, p[0], p[1])
		}
	}
	if !reflect.DeepEqual(plain(got), plain(want)) {
		t.Errorf("%s:\n got  %+v\n want %+v", what, plain(got), plain(want))
	}
}

func plain(t domain.Task) domain.Task {
	t.Deadline, t.KeepUntil, t.CreatedAt, t.EndedAt = time.Time{}, time.Time{}, time.Time{}, time.Time{}
	if len(t.Pins) == 0 {
		t.Pins = nil
	}
	if len(t.Refs) == 0 {
		t.Refs = nil
	}
	if t.Trigger != nil {
		tr := *t.Trigger
		if len(tr.Context) == 0 {
			tr.Context = nil
		}
		t.Trigger = &tr
	}
	return t
}

func sameEvents(t *testing.T, what string, got, want []domain.EventRow) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("%s: %d rows, want %d", what, len(got), len(want))
	}
	for i := range want {
		g, w := got[i], want[i]
		if g.Task != w.Task || g.Seq != w.Seq || g.Kind != w.Kind || string(g.Data) != string(w.Data) || !g.At.Equal(w.At) {
			t.Errorf("%s: row %d = %+v, want %+v", what, i, g, w)
		}
	}
}
