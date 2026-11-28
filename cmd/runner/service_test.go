package main

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestAUnitQuotesEveryArgumentSoSystemdReadsThemBack(t *testing.T) {
	u := unit{name: "moor-runner-m-1.service", description: "moor runner m-1", argv: []string{"/opt/my runner/runner", "-key", "/home/a b/k%d$HOME\"q\\.key"}}
	text := u.text()
	want := `ExecStart="/opt/my runner/runner" "-key" "/home/a b/k%%d$$HOME\"q\\.key"`
	if !strings.Contains(text, want) {
		t.Fatalf("unit text:\n%s\nwant the line %s", text, want)
	}
	for _, line := range []string{"Restart=on-failure", "WantedBy=default.target", "Description=moor runner m-1"} {
		if !strings.Contains(text, line) {
			t.Errorf("the unit lacks %q:\n%s", line, text)
		}
	}
}

func TestAUnitNameIsMadeOnlyFromAnIDThatCannotLeaveTheUnitFolder(t *testing.T) {
	if _, err := unitFor("/x/runner", options{id: "../../etc/x"}); err == nil {
		t.Error("an id with slashes made a unit")
	}
	u, err := unitFor("/x/runner", options{id: "m-abc.1", portal: "wss://p/runner/link", keyFile: "/k"})
	if err != nil || u.name != "moor-runner-m-abc.1.service" {
		t.Fatalf("unit = %+v, %v", u, err)
	}
}

type recordedCtl struct {
	calls   []string
	failOn  string
	lingers int
}

func (c *recordedCtl) ctl(_ context.Context, args ...string) error {
	c.calls = append(c.calls, strings.Join(args, " "))
	if c.failOn != "" && args[0] == c.failOn {
		return errors.New("no bus")
	}
	return nil
}

func (c *recordedCtl) linger(context.Context) error { c.lingers++; return nil }

func installOnce(t *testing.T, svc userService, u unit) string {
	t.Helper()
	got, err := svc.Install(context.Background(), u)
	if err != nil || got.Path != filepath.Join(svc.dir, u.name) || !got.Persistent {
		t.Fatalf("install = %+v, %v", got, err)
	}
	b, err := os.ReadFile(got.Path)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestInstallWritesTheUnitStartsItAndRepeatsWithoutChange(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "systemd", "user")
	rec := &recordedCtl{}
	svc := userService{dir: dir, ctl: rec.ctl, linger: rec.linger}
	u := unit{name: "moor-runner-m-1.service", description: "d", argv: []string{"/x/runner", "-id", "m-1"}}
	first := installOnce(t, svc, u)
	if again := installOnce(t, svc, u); again != first {
		t.Fatal("a repeat changed the unit file")
	}
	want := "show-environment|daemon-reload|enable moor-runner-m-1.service|restart moor-runner-m-1.service"
	if got := strings.Join(rec.calls[:4], "|"); got != want || len(rec.calls) != 8 || rec.lingers != 2 {
		t.Fatalf("systemctl calls %q, %d lingers", rec.calls, rec.lingers)
	}
}

func TestAHostWithNoUserManagerIsTold(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "user")
	rec := &recordedCtl{failOn: "show-environment"}
	_, err := userService{dir: dir, ctl: rec.ctl, linger: rec.linger}.Install(context.Background(), unit{name: "moor-runner-m-1.service", argv: []string{"/x"}})
	if !errors.Is(err, errNoService) {
		t.Fatalf("err = %v, want errNoService", err)
	}
	if _, statErr := os.Stat(dir); statErr == nil {
		t.Error("a host without a user manager got a unit folder")
	}
}
