package e2e

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
)

const keepWindow = 12 * time.Second

func TestAnEndedTaskKeepsItsContainersForTheWindowThenThePortalReleasesThemAndRecordsIt(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	prefix := "moore2e-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn, env: []string{"PORTAL_KEEP_WINDOW=" + keepWindow.String()}}
	pt.start(t)
	id, _ := runningOn(t, pt, build(t, dir, "cmd/runner"), dir, prefix)
	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()

	pt.call(t, "POST", "/api/tasks/"+string(id)+"/end", nil, nil)
	ended := time.Now()
	time.Sleep(keepWindow / 4)
	if left := remaining(t, prefix); len(left["container"]) != 5 {
		t.Fatalf("inside the keep window the host has %v, want the task's five containers", left)
	}
	if releases(t, db, id) != 0 {
		t.Fatal("a release was recorded inside the keep window")
	}
	eventually(t, time.Minute, "the ended task's containers, networks and volumes released", func() bool { return len(remaining(t, prefix)) == 0 })
	if kept := time.Since(ended); kept < keepWindow-2*time.Second {
		t.Errorf("the containers were removed after %s, inside the %s window", kept, keepWindow)
	}
	if n := releases(t, db, id); n != 1 {
		t.Errorf("%d release events, want 1", n)
	}
	if got, _ := db.Load(context.Background(), id); got.State != domain.Done {
		t.Errorf("the release moved the task to %s", got.State)
	}
}

// releases counts the rows that say a task's keep window ended.
func releases(t *testing.T, db *store.Store, id domain.TaskID) int {
	t.Helper()
	rows, err := db.EventsAfter(context.Background(), id, 0, 1000)
	must(t, err)
	n := 0
	for _, r := range rows {
		if r.Kind == domain.KindNote && strings.Contains(string(r.Data), domain.PhaseReleased) {
			n++
		}
	}
	return n
}
