package storetest

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// resultPages: a page is stored with its version in one write, read back by version, and gone with a refused write.
func resultPages(t *testing.T, s Subject) {
	ctx := context.Background()
	cur := create(t, s, person("t1"))
	for n, page := range []string{"<h1>one</h1>", "<h1>two</h1>"} {
		r := domain.ResultVersion{N: n + 1, EntryURL: "u", Line: "l", Digest: page, PublishedAt: at(int64(n))}
		saved, err := s.Store.Save(ctx, domain.TaskWrite{Task: cur, Result: &r, Page: []byte(page)})
		if err != nil {
			t.Fatal(err)
		}
		cur = saved
	}
	readPages(t, s)
	dup := domain.ResultVersion{N: 3, Digest: "<h1>one</h1>"}
	_, err := s.Store.Save(ctx, domain.TaskWrite{Task: cur, Result: &dup, Page: []byte("<h1>three</h1>")})
	wantErr(t, "the same digest", err, domain.ErrExists)
	if _, err := s.Page(ctx, "t1", 3); err == nil {
		t.Error("a refused write left its page")
	}
}

// resultWithoutAPage: a version published with no page has none to read.
func resultWithoutAPage(t *testing.T, s Subject) {
	ctx := context.Background()
	cur := create(t, s, person("t1"))
	if _, err := s.Store.Save(ctx, domain.TaskWrite{Task: cur, Result: &domain.ResultVersion{N: 1, Digest: "d"}}); err != nil {
		t.Fatal(err)
	}
	_, err := s.Page(ctx, "t1", 1)
	wantErr(t, "a version with no page", err, domain.ErrNotFound)
}

// notes: HasNote finds a note row by its exact text, in the task's own rows only.
func notes(t *testing.T, s Subject) {
	ctx := context.Background()
	task := person("t1")
	task.LastSeq = 3
	rows := eventsFrom("t1", 1, 3)
	rows[1].Data = []byte(`{"text":"desk up: m-1"}`)
	rows[2].Kind, rows[2].Data = domain.KindError, []byte(`{"text":"box up"}`)
	if _, err := s.Store.Save(ctx, domain.TaskWrite{Task: task, Create: true, Events: rows}); err != nil {
		t.Fatal(err)
	}
	create(t, s, person("t2"))
	for text, want := range map[string]bool{"x": true, "desk up: m-1": true, "desk up": false, "box up": false, "": false} {
		if got, err := s.Store.HasNote(ctx, "t1", text); err != nil || got != want {
			t.Errorf("HasNote(%q) = %v, %v; want %v", text, got, err, want)
		}
	}
	if got, err := s.Store.HasNote(ctx, "t2", "desk up: m-1"); err != nil || got {
		t.Errorf("another task's note was found: %v, %v", got, err)
	}
}

func readPages(t *testing.T, s Subject) {
	ctx := context.Background()
	for n, want := range []string{"<h1>one</h1>", "<h1>two</h1>"} {
		got, err := s.Page(ctx, "t1", n+1)
		if err != nil || string(got) != want {
			t.Errorf("page %d = %q, %v", n+1, got, err)
		}
	}
	for _, n := range []int{0, 3} {
		_, err := s.Page(ctx, "t1", n)
		wantErr(t, "a version that does not exist", err, domain.ErrNotFound)
	}
	_, err := s.Page(ctx, "nope", 1)
	wantErr(t, "a task that does not exist", err, domain.ErrNotFound)
}
