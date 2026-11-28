package main

import (
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

func TestTheKeepWindowDefaultsToADayAndTakesAFlagOrItsVariable(t *testing.T) {
	c, err := parseConfig(nil, env(goodEnv()))
	if err != nil || c.keepWindow != 24*time.Hour {
		t.Fatalf("default keep window = %v, %v", c.keepWindow, err)
	}
	e := goodEnv()
	e["PORTAL_KEEP_WINDOW"] = "6h"
	if c, err = parseConfig(nil, env(e)); err != nil || c.keepWindow != 6*time.Hour {
		t.Fatalf("keep window from the variable = %v, %v", c.keepWindow, err)
	}
	if c, err = parseConfig([]string{"-keep-window", "90s"}, env(e)); err != nil || c.keepWindow != 90*time.Second {
		t.Fatalf("keep window from the flag = %v, %v", c.keepWindow, err)
	}
}

func TestAKeepWindowThatIsNotAPositiveDurationIsRefused(t *testing.T) {
	for _, bad := range []string{"0", "0s", "-1h", "tomorrow"} {
		if _, err := parseConfig([]string{"-keep-window", bad}, env(goodEnv())); err == nil {
			t.Errorf("keep window %q was accepted", bad)
		}
	}
}

func TestTheTaskServiceGetsTheKeepWindowAndTheTimeLimit(t *testing.T) {
	in := task.Config{Answerable: map[domain.EventSource]bool{"github": true}}
	got := withOperations(in, config{keepWindow: time.Hour, timeLimit: time.Minute})
	if got.KeepWindow != time.Hour || got.TimeLimit != time.Minute || !got.Answerable["github"] {
		t.Fatalf("task config = %+v", got)
	}
}

func TestTheTimeLimitDefaultsToFourHoursAndTakesAFlagOrItsVariable(t *testing.T) {
	c, err := parseConfig(nil, env(goodEnv()))
	if err != nil || c.timeLimit != 4*time.Hour {
		t.Fatalf("default time limit = %v, %v", c.timeLimit, err)
	}
	e := goodEnv()
	e["PORTAL_TIME_LIMIT"] = "90m"
	if c, err = parseConfig(nil, env(e)); err != nil || c.timeLimit != 90*time.Minute {
		t.Fatalf("time limit from the variable = %v, %v", c.timeLimit, err)
	}
	if c, err = parseConfig([]string{"-time-limit", "30s"}, env(e)); err != nil || c.timeLimit != 30*time.Second {
		t.Fatalf("time limit from the flag = %v, %v", c.timeLimit, err)
	}
	for _, bad := range []string{"0", "-1h", "soon"} {
		if _, err := parseConfig([]string{"-time-limit", bad}, env(goodEnv())); err == nil {
			t.Errorf("time limit %q was accepted", bad)
		}
	}
}
