package store

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/domain/storetest"
)

func TestPostgresPassesTheStoreContract(t *testing.T) {
	storetest.Run(t, subject)
}

func newTask(id string, rev domain.Revision) domain.Task {
	tr := &domain.Trigger{Source: "forge", Group: "g", Rev: rev}
	return domain.Task{ID: domain.TaskID(id), Project: "shop", Trigger: tr, State: domain.Waiting, LastSeq: 1, CreatedAt: time.Unix(1, 0)}
}

func row(id domain.TaskID, seq int64) domain.EventRow {
	return domain.EventRow{Task: id, Seq: seq, At: time.Unix(seq, 0), Kind: domain.KindNote, Data: json.RawMessage(`{"n":1}`)}
}

func mustCreate(t *testing.T, s *Store, task domain.Task) domain.Task {
	t.Helper()
	saved, err := s.Save(context.Background(), domain.TaskWrite{Task: task, Create: true, Events: []domain.EventRow{row(task.ID, 1)}})
	if err != nil {
		t.Fatal(err)
	}
	return saved
}

func assertSeqs(t *testing.T, s *Store, id domain.TaskID, last int64) {
	t.Helper()
	rows, err := s.events(context.Background(), id)
	if err != nil {
		t.Fatal(err)
	}
	if int64(len(rows)) != last {
		t.Fatalf("task %s has %d events, want %d", id, len(rows), last)
	}
	for i, r := range rows {
		if r.Seq != int64(i+1) {
			t.Fatalf("task %s event %d has seq %d", id, i, r.Seq)
		}
	}
}

func race(n int, f func(i int) error) []error {
	errs := make([]error, n)
	start := make(chan struct{})
	var wg sync.WaitGroup
	for i := range n {
		wg.Go(func() {
			<-start
			errs[i] = f(i)
		})
	}
	close(start)
	wg.Wait()
	return errs
}

func TestTwoWritersOfOneTaskHaveOneWinner(t *testing.T) {
	s := freshStore(t)
	ctx := context.Background()
	for round := range 25 {
		id := fmt.Sprintf("t%d", round)
		cur := mustCreate(t, s, newTask(id, domain.Revision(id)))
		errs := race(2, func(i int) error {
			next := cur
			next.LastSeq, next.Run = 2, int64(i+1)
			_, err := s.Save(ctx, domain.TaskWrite{Task: next, Events: []domain.EventRow{row(cur.ID, 2)}})
			return err
		})
		wins := 0
		for _, err := range errs {
			switch {
			case err == nil:
				wins++
			case !errors.Is(err, domain.ErrStaleVersion):
				t.Fatalf("round %d: %v", round, err)
			}
		}
		if wins != 1 {
			t.Fatalf("round %d: %d writers won, want 1 (%v)", round, wins, errs)
		}
		assertSeqs(t, s, cur.ID, 2)
	}
}

// appendEvent adds one event on top of what it loaded and reports false when it lost the race.
func appendEvent(ctx context.Context, s *Store, id domain.TaskID) (bool, error) {
	cur, err := s.Load(ctx, id)
	if err != nil {
		return false, fmt.Errorf("load: %w", err)
	}
	cur.LastSeq++
	_, err = s.Save(ctx, domain.TaskWrite{Task: cur, Events: []domain.EventRow{row(id, cur.LastSeq)}})
	if errors.Is(err, domain.ErrStaleVersion) {
		return false, nil
	}
	return err == nil, err
}

func appendUntil(ctx context.Context, s *Store, id domain.TaskID, want int) error {
	for done := 0; done < want; {
		ok, err := appendEvent(ctx, s, id)
		if err != nil {
			return err
		}
		if ok {
			done++
		}
	}
	return nil
}

func TestRetryingWritersLeaveNoGapInSeq(t *testing.T) {
	const writers, each = 8, 5
	s := freshStore(t)
	ctx := context.Background()
	mustCreate(t, s, newTask("t1", "r1"))
	errs := race(writers, func(int) error { return appendUntil(ctx, s, "t1", each) })
	for _, err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	assertSeqs(t, s, "t1", 1+writers*each)
	if got, _ := s.Load(ctx, "t1"); got.Version != 1+writers*each || got.LastSeq != 1+writers*each {
		t.Errorf("version %d, last_seq %d, want %d", got.Version, got.LastSeq, 1+writers*each)
	}
}

func TestDifferentTasksDoNotWaitForEachOther(t *testing.T) {
	s := freshStore(t)
	errs := race(30, func(i int) error {
		id := domain.TaskID(fmt.Sprintf("t%d", i))
		task := newTask(string(id), domain.Revision(id))
		_, err := s.Save(context.Background(), domain.TaskWrite{Task: task, Create: true, Events: []domain.EventRow{row(id, 1)}})
		return err
	})
	for _, err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	if n := count(t, s, "SELECT count(*) FROM tasks"); n != 30 {
		t.Errorf("%d tasks, want 30", n)
	}
}

func TestFiftyStartsWithOneKeyCreateOneTask(t *testing.T) {
	s := freshStore(t)
	errs := race(50, func(i int) error {
		id := domain.TaskID(fmt.Sprintf("t%d", i))
		task := newTask(string(id), "same-rev")
		_, err := s.Save(context.Background(), domain.TaskWrite{Task: task, Create: true, Events: []domain.EventRow{row(id, 1)}})
		return err
	})
	wins := 0
	for _, err := range errs {
		switch {
		case err == nil:
			wins++
		case !errors.Is(err, domain.ErrExists):
			t.Fatalf("a losing start got %v, want ErrExists", err)
		}
	}
	if wins != 1 {
		t.Fatalf("%d starts won, want 1", wins)
	}
	if n := count(t, s, "SELECT count(*) FROM tasks"); n != 1 {
		t.Errorf("%d tasks, want 1", n)
	}
	if n := count(t, s, "SELECT count(*) FROM task_events"); n != 1 {
		t.Errorf("%d events, want 1", n)
	}
}

func TestFailedSaveLeavesNoPartialRows(t *testing.T) {
	s := freshStore(t)
	ctx := context.Background()
	cur := mustCreate(t, s, newTask("t1", "r1"))
	next := cur
	next.State, next.LastSeq = domain.Running, 3
	bad := row("t1", 3)
	bad.Data = json.RawMessage("{not json")
	w := domain.TaskWrite{Task: next, Events: []domain.EventRow{row("t1", 2), bad}, Result: &domain.ResultVersion{N: 1, Digest: "d"}}
	if _, err := s.Save(ctx, w); err == nil {
		t.Fatal("a write with an invalid event was stored")
	}
	got, err := s.Load(ctx, "t1")
	if err != nil || got.State != domain.Waiting || got.Version != 1 || got.LastSeq != 1 {
		t.Errorf("task after the failed save: %+v, %v", got, err)
	}
	assertSeqs(t, s, "t1", 1)
	if n := count(t, s, "SELECT count(*) FROM result_versions"); n != 0 {
		t.Errorf("%d result versions, want 0", n)
	}
	w.Events[1] = row("t1", 3)
	if _, err := s.Save(ctx, w); err != nil {
		t.Errorf("the same write with valid events: %v", err)
	}
}

func TestOpeningTwiceAppliesMigrationsOnce(t *testing.T) {
	dsn := schemaDSN(t)
	errs := race(6, func(int) error {
		s, err := Open(context.Background(), dsn)
		if err == nil {
			s.Close()
		}
		return err
	})
	for _, err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	s := openStore(t, dsn)
	if n := count(t, s, "SELECT count(*) FROM goose_db_version WHERE version_id = 1"); n != 1 {
		t.Errorf("migration 1 recorded %d times, want 1", n)
	}
}
