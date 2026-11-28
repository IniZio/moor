package main

import (
	"net/http"
	"testing"
	"time"
)

func TestReadinessReportsTheProcessAndNeverAsksTheDatabase(t *testing.T) {
	a := newApp(t)
	a.do(get("", "/readyz")).has(t, http.StatusOK)
	a.close()
	a.do(get("", "/readyz")).has(t, http.StatusOK)
	a.do(get("", "/healthz")).has(t, http.StatusOK)
}

func TestTheShutdownDrainIsTheSpecsThirtySeconds(t *testing.T) {
	if shutdownGrace != 30*time.Second {
		t.Fatalf("the drain is %s, the stop rule says 30s", shutdownGrace)
	}
}
