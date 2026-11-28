package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestEveryStateHasItsGroup(t *testing.T) {
	want := map[domain.State]domain.StateGroup{
		domain.Waiting: domain.GroupQueued, domain.Starting: domain.GroupRunning, domain.Running: domain.GroupRunning,
		domain.Delivering: domain.GroupRunning, domain.Done: domain.GroupDone, domain.Failed: domain.GroupFailed,
		domain.Cancelled: domain.GroupCancelled, domain.Superseded: domain.GroupCancelled,
	}
	for s := domain.Waiting; s <= domain.Cancelled; s++ {
		if got := s.Group(); got != want[s] {
			t.Errorf("%v is in %q, want %q", s, got, want[s])
		}
	}
}

func TestTheFiveGroupsListEveryStateOnce(t *testing.T) {
	if len(domain.StateGroups) != 5 {
		t.Errorf("%d groups, want the five icons", len(domain.StateGroups))
	}
	listed := map[domain.State]int{}
	for _, g := range domain.StateGroups {
		for _, s := range g.States() {
			listed[s]++
		}
	}
	for s := domain.Waiting; s <= domain.Cancelled; s++ {
		if listed[s] != 1 {
			t.Errorf("%v is listed by %d groups, want 1", s, listed[s])
		}
	}
}

func TestParseStateGroup(t *testing.T) {
	for _, g := range domain.StateGroups {
		got, err := domain.ParseStateGroup(string(g))
		if err != nil || got != g {
			t.Errorf("%q: %v, %v", g, got, err)
		}
	}
	for _, bad := range []string{"", "waiting", "Done", "superseded"} {
		if _, err := domain.ParseStateGroup(bad); !errors.Is(err, domain.ErrUnknownState) {
			t.Errorf("%q: err = %v", bad, err)
		}
	}
}
