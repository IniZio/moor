package store

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

func TestADeliveryIsStoredOnceByItsSourceAndId(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	at := time.Date(2026, 11, 28, 9, 0, 0, 0, time.UTC)
	ev := domain.InboundEvent{Source: "github", DeliveryID: "d-1", Event: "pull_request", Action: "opened", ReceivedAt: at, Outcome: "received"}
	if first, err := s.RecordInbound(ctx, ev); err != nil || !first {
		t.Fatalf("first record: %v, %v", first, err)
	}
	ev.Outcome = "changed"
	if first, err := s.RecordInbound(ctx, ev); err != nil || first {
		t.Fatalf("second record: %v, %v", first, err)
	}
	if first, err := s.RecordInbound(ctx, domain.InboundEvent{Source: "other", DeliveryID: "d-1", Event: "e", ReceivedAt: at}); err != nil || !first {
		t.Fatalf("the same id from another source: %v, %v", first, err)
	}
	got, ids, err := s.Inbound(ctx, "github", "d-1")
	if err != nil || got.Outcome != "received" || got.Action != "opened" || !got.ReceivedAt.Equal(at) || len(ids) != 0 {
		t.Errorf("read %+v %v %v", got, ids, err)
	}
	if _, _, err := s.Inbound(ctx, "github", "nope"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("an unknown delivery: %v", err)
	}
}

func TestADeliveryKnowsTheTasksItStartedInTheOrderTheyWereMade(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	for _, id := range []string{"1", "2"} {
		if _, err := s.Save(ctx, domain.TaskWrite{Task: newTask(id, domain.Revision("r"+id)), Create: true, Events: []domain.EventRow{{Task: domain.TaskID(id), Seq: 1, At: time.Unix(1, 0), Kind: domain.KindNote}}}); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := s.RecordInbound(ctx, domain.InboundEvent{Source: "github", DeliveryID: "d-1", Event: "e", ReceivedAt: time.Unix(1, 0)}); err != nil {
		t.Fatal(err)
	}
	for _, id := range []domain.TaskID{"2", "1", "2"} {
		if err := s.LinkInbound(ctx, "github", "d-1", id); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.FinishInbound(ctx, "github", "d-1", "started 2 tasks"); err != nil {
		t.Fatal(err)
	}
	got, ids, err := s.Inbound(ctx, "github", "d-1")
	if err != nil || got.Outcome != "started 2 tasks" || len(ids) != 2 {
		t.Fatalf("read %+v %v %v", got, ids, err)
	}
}

func TestATransactionKeepsAnEventAndItsTasksTogetherOrNotAtAll(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	boom := errors.New("boom")
	err := s.InTx(ctx, func(tx *Store) error {
		if _, err := tx.RecordInbound(ctx, domain.InboundEvent{Source: "github", DeliveryID: "d-1", Event: "e", ReceivedAt: time.Unix(1, 0)}); err != nil {
			return err
		}
		if _, err := tx.Save(ctx, domain.TaskWrite{Task: newTask("1", "r"), Create: true, Events: []domain.EventRow{{Task: "1", Seq: 1, At: time.Unix(1, 0), Kind: domain.KindNote}}}); err != nil {
			return err
		}
		return boom
	})
	if !errors.Is(err, boom) {
		t.Fatalf("err = %v", err)
	}
	if _, _, err := s.Inbound(ctx, "github", "d-1"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("the delivery stayed: %v", err)
	}
	if _, err := s.Load(ctx, "1"); !errors.Is(err, domain.ErrNotFound) {
		t.Errorf("the task stayed: %v", err)
	}
}

func TestACreateThatLosesToTheKeyInsideATransactionLeavesTheTransactionUsable(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	if _, err := s.Save(ctx, domain.TaskWrite{Task: newTask("1", "r"), Create: true, Events: []domain.EventRow{{Task: "1", Seq: 1, At: time.Unix(1, 0), Kind: domain.KindNote}}}); err != nil {
		t.Fatal(err)
	}
	err := s.InTx(ctx, func(tx *Store) error {
		_, err := tx.Save(ctx, domain.TaskWrite{Task: newTask("2", "r"), Create: true, Events: []domain.EventRow{{Task: "2", Seq: 1, At: time.Unix(1, 0), Kind: domain.KindNote}}})
		if !errors.Is(err, domain.ErrExists) {
			t.Errorf("the second create: %v", err)
		}
		_, err = tx.Load(ctx, "1")
		return err
	})
	if err != nil {
		t.Errorf("the transaction could not go on: %v", err)
	}
}

// addSection adds the caller's own section to the delivery of a place, slowly, so callers that did not take turns would overwrite each other.
func addSection(s *Store, i int) error {
	return s.Locked(context.Background(), "k", "example-org/shop-web#90", func(d domain.Delivery) (domain.Delivery, error) {
		time.Sleep(5 * time.Millisecond)
		if d.Sections == nil {
			d.ExternalID, d.Sections = "c-1", map[domain.SpecName]domain.Section{}
		}
		d.Sections[domain.SpecName(fmt.Sprintf("spec-%d", i))] = domain.Section{Version: i}
		return d, nil
	})
}

func TestCallersOfOnePlaceTakeTurnsSoNoWriteIsLost(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	const callers = 8
	var wg sync.WaitGroup
	for i := range callers {
		wg.Go(func() {
			if err := addSection(s, i); err != nil {
				t.Error(err)
			}
		})
	}
	wg.Wait()
	err := s.Locked(ctx, "k", "example-org/shop-web#90", func(d domain.Delivery) (domain.Delivery, error) {
		if d.ExternalID != "c-1" || len(d.Sections) != callers {
			t.Errorf("delivery = %+v", d)
		}
		return d, nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func TestAFailedCallLeavesTheDeliveryAsItWas(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	boom := errors.New("boom")
	err := s.Locked(ctx, "k", "r", func(d domain.Delivery) (domain.Delivery, error) {
		d.ExternalID = "c-1"
		return d, boom
	})
	if !errors.Is(err, boom) {
		t.Fatal(err)
	}
	if err := s.Locked(ctx, "k", "r", func(d domain.Delivery) (domain.Delivery, error) {
		if d.ExternalID != "" {
			t.Errorf("delivery = %+v", d)
		}
		return d, nil
	}); err != nil {
		t.Fatal(err)
	}
}
