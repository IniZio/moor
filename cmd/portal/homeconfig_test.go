package main

import (
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

func TestTheHomeRepositoryComesFromConfigurationWithItsBranchAndRefresh(t *testing.T) {
	c, err := parseConfig(nil, env(goodEnv()))
	if err != nil || c.home.repo.Complete() || c.home.every != 5*time.Minute {
		t.Fatalf("default home = %+v, %v", c.home, err)
	}
	e := goodEnv()
	e["PORTAL_HOME_REPO"] = "github.com/Example-Org/team-home"
	e["PORTAL_HOME_REFRESH"] = "90s"
	c, err = parseConfig(nil, env(e))
	if err != nil || c.home.repo != (domain.Repo{Host: "github.com", Path: "example-org/team-home"}) || c.home.branch != "" || c.home.every != 90*time.Second {
		t.Fatalf("home from the environment = %+v, %v", c.home, err)
	}
	c, err = parseConfig([]string{"-home-repo", "github.com/example-org/team-home@release", "-home-refresh", "1m"}, env(goodEnv()))
	if err != nil || c.home.branch != "release" || c.home.every != time.Minute {
		t.Fatalf("home from the flags = %+v, %v", c.home, err)
	}
}

func TestAHomeRepositoryAndAHomeFolderAreExclusive(t *testing.T) {
	e := goodEnv()
	e["PORTAL_HOME_REPO"] = "github.com/example-org/team-home"
	e["PORTAL_HOME_DIR"] = "/srv/home"
	if _, err := parseConfig(nil, env(e)); err == nil || !strings.Contains(err.Error(), "exclusive") {
		t.Fatalf("both set gave %v", err)
	}
}

func TestABadHomeRepositoryIsRefusedWithItsReason(t *testing.T) {
	for _, bad := range []string{"team-home", "gitlab.example.com/example-org/team-home", "github.com/example-org/team/home", "github.com/example-org/team-home@"} {
		e := goodEnv()
		e["PORTAL_HOME_REPO"] = bad
		if _, err := parseConfig(nil, env(e)); err == nil || !strings.Contains(err.Error(), "home-repo") {
			t.Errorf("%q gave %v", bad, err)
		}
	}
	e := goodEnv()
	e["PORTAL_HOME_REFRESH"] = "soon"
	if _, err := parseConfig(nil, env(e)); err == nil || !strings.Contains(err.Error(), "home-refresh") {
		t.Errorf("a bad refresh gave %v", err)
	}
}
