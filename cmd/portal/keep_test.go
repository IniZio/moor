package main

import (
	"testing"
	"time"

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

func TestTheTaskServiceGetsTheKeepWindow(t *testing.T) {
	if got := withKeep(task.Config{TimeLimit: time.Minute}, config{keepWindow: time.Hour}); got.KeepWindow != time.Hour || got.TimeLimit != time.Minute {
		t.Fatalf("task config = %+v", got)
	}
}
