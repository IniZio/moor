package storetest

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// BlockedStore is the calls the settings logic makes on blocked requests.
type BlockedStore interface {
	RecordBlocked(ctx context.Context, h domain.BlockedHit) error
	// BlockedRequests lists the task's rows, oldest first.
	BlockedRequests(ctx context.Context, task domain.TaskID) ([]domain.BlockedRequest, error)
	// BlockedRequest returns domain.ErrNotFound for an id no row has.
	BlockedRequest(ctx context.Context, id domain.BlockedID) (domain.BlockedRequest, error)
	DecideBlocked(ctx context.Context, id domain.BlockedID, d domain.Decision, by string, at time.Time) error
}

// RunBlocked runs the blocked request cases on a fresh store.
func RunBlocked(t *testing.T, fresh func(t *testing.T) BlockedStore) {
	t.Helper()
	t.Run("a repeat counts on one row", func(t *testing.T) { blockedCounts(t, fresh(t)) })
	t.Run("a decision is kept", func(t *testing.T) { blockedDecision(t, fresh(t)) })
}

var blockedAt = time.Date(2026, 11, 28, 10, 0, 0, 0, time.UTC)

func blockedHit(task domain.TaskID, proxy domain.ProxyKind, host string, method domain.Method, at time.Time) domain.BlockedHit {
	return domain.BlockedHit{Task: task, Proxy: proxy, Host: host, Port: 443, Method: method, Reason: domain.DenyHost, At: at}
}

func record(t *testing.T, s BlockedStore, hits ...domain.BlockedHit) {
	t.Helper()
	for _, h := range hits {
		if err := s.RecordBlocked(context.Background(), h); err != nil {
			t.Fatal(err)
		}
	}
}

func blockedCounts(t *testing.T, s BlockedStore) {
	later := blockedAt.Add(time.Minute)
	record(t, s,
		blockedHit("1", domain.BoxProxy, "fonts.example.com", "GET", blockedAt),
		blockedHit("1", domain.BoxProxy, "fonts.example.com", "GET", later),
		blockedHit("1", domain.BoxProxy, "fonts.example.com", "POST", later),
		blockedHit("1", domain.DeskProxy, "fonts.example.com", "GET", later),
		blockedHit("2", domain.BoxProxy, "fonts.example.com", "GET", later),
	)
	got, err := s.BlockedRequests(context.Background(), "1")
	if err != nil || len(got) != 3 {
		t.Fatalf("task 1 has %d rows: %+v %v", len(got), got, err)
	}
	first := got[0]
	if first.Request() != "fonts.example.com:443 GET" || first.Proxy != domain.BoxProxy || first.Count != 2 ||
		!first.FirstAt.Equal(blockedAt) || !first.LastAt.Equal(later) || first.Decision != domain.Undecided {
		t.Fatalf("the repeated row is %+v", first)
	}
	one, err := s.BlockedRequest(context.Background(), first.ID)
	if err != nil || one.ID != first.ID || one.Task != "1" {
		t.Fatalf("by id: %+v %v", one, err)
	}
	if _, err := s.BlockedRequest(context.Background(), first.ID+1000); !errors.Is(err, domain.ErrNotFound) {
		t.Fatalf("an unknown id: %v", err)
	}
}

func blockedDecision(t *testing.T, s BlockedStore) {
	record(t, s, blockedHit("1", domain.BoxProxy, "fonts.example.com", "GET", blockedAt))
	got, _ := s.BlockedRequests(context.Background(), "1")
	if err := s.DecideBlocked(context.Background(), got[0].ID, domain.Allowed, "Mia", blockedAt.Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	record(t, s, blockedHit("1", domain.BoxProxy, "fonts.example.com", "GET", blockedAt.Add(2*time.Hour)))
	got, _ = s.BlockedRequests(context.Background(), "1")
	b := got[0]
	if b.Decision != domain.Allowed || b.DecidedBy != "Mia" || !b.DecidedAt.Equal(blockedAt.Add(time.Hour)) || b.Count != 2 {
		t.Fatalf("after a decision and a late repeat the row is %+v", b)
	}
	if err := s.DecideBlocked(context.Background(), b.ID+1000, domain.Dismissed, "Mia", blockedAt); !errors.Is(err, domain.ErrNotFound) {
		t.Fatalf("deciding an unknown id: %v", err)
	}
}
