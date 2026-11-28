package domain_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestLiveStates(t *testing.T) {
	want := map[domain.State]bool{
		domain.Waiting: true, domain.Starting: true, domain.Running: true, domain.Delivering: true,
		domain.Done: false, domain.Failed: false, domain.Superseded: false, domain.Cancelled: false,
	}
	for _, s := range allStates {
		if s.Live() != want[s] {
			t.Errorf("%s.Live() = %v", s, s.Live())
		}
	}
}

func allowedSomewhere(flow domain.Flow, ev domain.Signal) bool {
	for _, from := range allStates {
		if _, err := domain.Next(flow, from, ev); err == nil {
			return true
		}
	}
	return false
}

func TestHandlesMatchesNext(t *testing.T) {
	for _, flow := range allFlows {
		for _, ev := range allSignals {
			if got, want := domain.Handles(flow, ev), allowedSomewhere(flow, ev); got != want {
				t.Errorf("Handles(%s, %s) = %v, but Next allows it somewhere: %v", flow, ev, got, want)
			}
		}
	}
}
