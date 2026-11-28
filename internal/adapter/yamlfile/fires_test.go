package yamlfile_test

import (
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
	"github.com/IniZio/moor/internal/domain"
)

// firings lists the local times at which the entry fires in the UTC minutes from start, for n minutes.
func firings(s domain.ScheduleOn, start time.Time, n int) []string {
	var out []string
	for i := range n {
		if local, ok := (yamlfile.Crons{}).Fires(s, start.Add(time.Duration(i)*time.Minute)); ok {
			out = append(out, local.Format("2006-01-02T15:04 MST"))
		}
	}
	return out
}

func TestAScheduleEntryTakesAnIANATimeZone(t *testing.T) {
	spec := func(zone string) string {
		return "name: a\non:\n  schedule:\n    - cron: 0 6 * * 1\n      timezone: " + zone + "\nprompt: p\n"
	}
	got, err := yamlfile.ParseTaskSpec([]byte(spec("Europe/London")))
	if err != nil || len(got.On.Schedule) != 1 || got.On.Schedule[0].Timezone != "Europe/London" {
		t.Fatalf("spec %+v, %v", got.On, err)
	}
	for _, bad := range []string{"Mars/Olympus", "Local", "+08:00"} {
		if _, err := yamlfile.ParseTaskSpec([]byte(spec(bad))); err == nil {
			t.Errorf("timezone %q was taken", bad)
		}
	}
}

func TestACronFiresInTheMinutesItNamesInUTC(t *testing.T) {
	monday := time.Date(2026, 11, 30, 5, 58, 0, 0, time.UTC)
	got := firings(domain.ScheduleOn{Cron: "0 6 * * 1"}, monday, 5)
	if len(got) != 1 || got[0] != "2026-11-30T06:00 UTC" {
		t.Errorf("firings %v", got)
	}
	if got := firings(domain.ScheduleOn{Cron: "0 6 * * 2"}, monday, 5); len(got) != 0 {
		t.Errorf("a Tuesday cron fired on a Monday: %v", got)
	}
	late := time.Date(2026, 11, 30, 6, 0, 59, 0, time.UTC)
	if _, ok := (yamlfile.Crons{}).Fires(domain.ScheduleOn{Cron: "0 6 * * 1"}, late); !ok {
		t.Errorf("a wake-up late in the minute missed it")
	}
}

func TestACronInATimeZoneFiresAtItsLocalTime(t *testing.T) {
	got := firings(domain.ScheduleOn{Cron: "0 6 * * *", Timezone: "Asia/Tokyo"}, time.Date(2026, 11, 29, 20, 0, 0, 0, time.UTC), 24*60)
	if len(got) != 1 || got[0] != "2026-11-30T06:00 JST" {
		t.Errorf("firings %v", got)
	}
}

// On 2026-03-08 New York's clocks go from 02:00 to 03:00, and on 2026-11-01 from 02:00 back to 01:00.
func TestAroundAClockChangeASkippedMinuteDoesNotFireAndARepeatedOneComesTwice(t *testing.T) {
	ny := domain.ScheduleOn{Cron: "30 2 * * *", Timezone: "America/New_York"}
	if got := firings(ny, time.Date(2026, 3, 8, 5, 0, 0, 0, time.UTC), 4*60); len(got) != 0 {
		t.Errorf("a minute the clock skipped fired: %v", got)
	}
	repeated := domain.ScheduleOn{Cron: "30 1 * * *", Timezone: "America/New_York"}
	got := firings(repeated, time.Date(2026, 11, 1, 4, 0, 0, 0, time.UTC), 4*60)
	if len(got) != 2 || got[0] != "2026-11-01T01:30 EDT" || got[1] != "2026-11-01T01:30 EST" {
		t.Errorf("firings %v, want the repeated 01:30 twice, which the clock keys as one", got)
	}
}
