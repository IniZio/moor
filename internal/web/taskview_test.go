package web

import (
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

var noon = time.Date(2026, 11, 28, 12, 0, 0, 0, time.UTC)

func TestAgeIsWordedInTheLargestWholeUnit(t *testing.T) {
	for d, want := range map[time.Duration]string{
		-time.Minute: "just now", 0: "just now", 59 * time.Second: "just now", time.Minute: "1m ago", 12*time.Minute + 40*time.Second: "12m ago",
		59 * time.Minute: "59m ago", time.Hour: "1h ago", 23*time.Hour + 59*time.Minute: "23h ago", 24 * time.Hour: "1d ago", 4*24*time.Hour + 5*time.Hour: "4d ago",
	} {
		if got := ago(noon, noon.Add(-d)); got != want {
			t.Errorf("%v ago = %q, want %q", d, got, want)
		}
	}
}

func TestDurationIsWordedLikeTheMock(t *testing.T) {
	for d, want := range map[time.Duration]string{
		0: "0s", 40 * time.Second: "40s", 62 * time.Second: "1m 02s", 9*time.Minute + 41*time.Second: "9m 41s", time.Hour + 5*time.Minute: "1h 05m",
		26 * time.Hour: "1d 02h", 59*time.Second + 600*time.Millisecond: "1m 00s",
	} {
		if got := lasted(noon, noon.Add(d)); got != want {
			t.Errorf("%v = %q, want %q", d, got, want)
		}
	}
	if got := lasted(noon, noon.Add(-time.Hour)); got != "0s" {
		t.Errorf("a task that ends before it starts lasted %q", got)
	}
}

func TestACountSaysWhenTheListIsCut(t *testing.T) {
	for _, c := range []struct {
		n    int
		more bool
		want string
	}{{0, false, "0 tasks"}, {1, false, "1 task"}, {10, false, "10 tasks"}, {200, true, "Newest 200 tasks"}} {
		if got := countText(c.n, c.more); got != c.want {
			t.Errorf("%d %v = %q, want %q", c.n, c.more, got, c.want)
		}
	}
}

func TestARowTellsHowTheTaskStarted(t *testing.T) {
	spec := &domain.Spec{Name: "PR evidence", File: "tasks/pr-evidence.yaml"}
	for name, c := range map[string]struct {
		it   domain.TaskListing
		spec string
		tail string
	}{
		"a person":           {domain.TaskListing{Task: domain.Task{ID: "145", StartedBy: "Mia", Prompt: "Add a coupon\nmore"}}, "", "#145: started by Mia"},
		"an event":           {domain.TaskListing{Task: domain.Task{ID: "142", Spec: spec, StartedBy: "event:github"}}, "PR evidence", "#142: github event"},
		"Run by hand":        {domain.TaskListing{Task: domain.Task{ID: "134", Spec: spec, StartedBy: "Ken"}}, "PR evidence", "#134: Run by Ken"},
		"a superseded event": {domain.TaskListing{Task: domain.Task{ID: "141", Spec: spec, StartedBy: "event:github"}, ReplacedBy: "142"}, "PR evidence", "#141: github event, replaced by #142"},
	} {
		got := rowOf(c.it, noon)
		if got.Spec != c.spec || got.Tail != c.tail {
			t.Errorf("%s: spec %q tail %q, want %q %q", name, got.Spec, got.Tail, c.spec, c.tail)
		}
	}
}
