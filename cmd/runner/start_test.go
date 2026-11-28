package main

import (
	"bytes"
	"context"
	"errors"
	"net/http"
	"path/filepath"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/link"
)

// standIns is what join touches outside this process, recorded.
type standIns struct {
	out    bytes.Buffer
	svc    *fakeService
	served []options
	probes int
	probe  error
	joins  []joinSeen
}

type fakeService struct {
	units []unit
	err   error
}

func (f *fakeService) Install(_ context.Context, u unit) (installed, error) {
	f.units = append(f.units, u)
	return installed{Path: "/units/" + u.name, Persistent: true}, f.err
}

func (s *standIns) env() joinEnv {
	return joinEnv{
		out: &s.out, service: s.svc,
		probe: func(context.Context, link.AgentConfig) error { s.probes++; return s.probe },
		serve: func(_ context.Context, o options) error { s.served = append(s.served, o); return nil },
		exe:   func() (string, error) { return "/opt/moor/runner", nil },
	}
}

var imageFlags = []string{"-box-image", "b:1", "-desk-image", "d:1", "-relay-image", "r:1", "-proxy-image", "p:1"}

func joinArgs(portal, keyFile string, more ...string) []string {
	return append(append([]string{"-portal", portal, "-id", "m-1", "-token", "tok", "-key", keyFile, "-name", "host-a", "-prefix", "moor"}, imageFlags...), more...)
}

func newStandIns() *standIns { return &standIns{svc: &fakeService{}} }

func TestJoinEnrolsThenStartsTheRunnerAsAUserServiceAndSaysSo(t *testing.T) {
	s := newStandIns()
	url := joinPortal(t, http.StatusOK, &s.joins)
	keyFile := filepath.Join(t.TempDir(), "runner.key")
	if err := join(context.Background(), joinArgs(url, keyFile), s.env()); err != nil {
		t.Fatal(err)
	}
	if len(s.joins) != 1 || len(s.svc.units) != 1 || len(s.served) != 0 {
		t.Fatalf("%d joins, %d units installed, %d runs in the shell, want 1, 1 and 0", len(s.joins), len(s.svc.units), len(s.served))
	}
	u := s.svc.units[0]
	want := []string{"/opt/moor/runner", "-portal", "ws" + strings.TrimPrefix(url, "http") + "/runner/link", "-id", "m-1", "-key", keyFile, "-prefix", "moor", "-box-image", "b:1", "-desk-image", "d:1", "-relay-image", "r:1", "-proxy-image", "p:1"}
	if u.name != "moor-runner-m-1.service" || strings.Join(u.argv, "\n") != strings.Join(want, "\n") {
		t.Fatalf("unit %s runs %q, want %q", u.name, u.argv, want)
	}
	for _, say := range []string{"systemd user service", "moor-runner-m-1.service", "journalctl --user -u moor-runner-m-1.service", "keeps running after this shell closes"} {
		if !strings.Contains(s.out.String(), say) {
			t.Errorf("the output lacks %q:\n%s", say, s.out.String())
		}
	}
}

func TestJoinWithoutAUserServiceRunsTheRunnerInThisShellAndSaysItStopsWithIt(t *testing.T) {
	s := newStandIns()
	s.svc.err = errNoService
	url := joinPortal(t, http.StatusOK, &s.joins)
	if err := join(context.Background(), joinArgs(url, filepath.Join(t.TempDir(), "k")), s.env()); err != nil {
		t.Fatal(err)
	}
	if len(s.served) != 1 || s.served[0].id != "m-1" || !strings.HasPrefix(s.served[0].portal, "ws") || !strings.HasSuffix(s.served[0].portal, "/runner/link") {
		t.Fatalf("runs in the shell: %+v", s.served)
	}
	if !strings.Contains(s.out.String(), "stops when this shell closes") {
		t.Errorf("the output does not say the runner stops with the shell:\n%s", s.out.String())
	}
}

func TestForegroundRunsTheRunnerInThisProcessAndInstallsNothing(t *testing.T) {
	s := newStandIns()
	url := joinPortal(t, http.StatusOK, &s.joins)
	if err := join(context.Background(), joinArgs(url, filepath.Join(t.TempDir(), "k"), "-foreground"), s.env()); err != nil {
		t.Fatal(err)
	}
	if len(s.svc.units) != 0 || len(s.served) != 1 {
		t.Fatalf("%d units, %d runs in the shell, want 0 and 1", len(s.svc.units), len(s.served))
	}
}

func TestEnrolOnlyJoinsAndStartsNothing(t *testing.T) {
	s := newStandIns()
	url := joinPortal(t, http.StatusOK, &s.joins)
	if err := join(context.Background(), []string{"-enrol-only", "-portal", url, "-id", "m-1", "-token", "tok", "-key", filepath.Join(t.TempDir(), "k"), "-name", "h"}, s.env()); err != nil {
		t.Fatal(err)
	}
	if len(s.joins) != 1 || len(s.svc.units) != 0 || len(s.served) != 0 {
		t.Fatalf("%d joins, %d units, %d runs", len(s.joins), len(s.svc.units), len(s.served))
	}
}

func TestJoinAgainWithTheTokenBurnedStartsWhenThePortalKnowsTheKeyAndSaysWhyOtherwise(t *testing.T) {
	s := newStandIns()
	url := joinPortal(t, http.StatusForbidden, &s.joins)
	keyFile := filepath.Join(t.TempDir(), "runner.key")
	if err := join(context.Background(), joinArgs(url, keyFile), s.env()); err != nil {
		t.Fatalf("a join the portal refuses as used, with this key already enrolled: %v", err)
	}
	if s.probes != 1 || len(s.svc.units) != 1 {
		t.Fatalf("%d probes, %d units, want 1 and 1", s.probes, len(s.svc.units))
	}
	s = newStandIns()
	s.probe = errors.New("refused: unauthorized")
	err := join(context.Background(), joinArgs(url, filepath.Join(t.TempDir(), "other")), s.env())
	if err == nil || !strings.Contains(err.Error(), "unknown, used or expired") || len(s.svc.units) != 0 {
		t.Fatalf("a token that is used by someone else's key: err = %v, %d units", err, len(s.svc.units))
	}
}

func TestJoinNeedsTheImagesBeforeItSpendsTheToken(t *testing.T) {
	s := newStandIns()
	url := joinPortal(t, http.StatusOK, &s.joins)
	args := []string{"-portal", url, "-id", "m-1", "-token", "tok", "-key", filepath.Join(t.TempDir(), "k")}
	err := join(context.Background(), args, s.env())
	if err == nil || !strings.Contains(err.Error(), "-box-image") || len(s.joins) != 0 {
		t.Fatalf("err = %v with %d joins sent", err, len(s.joins))
	}
}

func TestTheLinkURLIsTheSchemeAndPathOfThePortalAddress(t *testing.T) {
	for in, want := range map[string]string{
		"https://portal.example.org":     "wss://portal.example.org/runner/link",
		"http://127.0.0.1:8080/":         "ws://127.0.0.1:8080/runner/link",
		"https://example.org/moor/":      "wss://example.org/moor/runner/link",
		"https://portal.example.org:443": "wss://portal.example.org:443/runner/link",
	} {
		if got, err := linkURL(in); err != nil || got != want {
			t.Errorf("linkURL(%q) = %q, %v, want %q", in, got, err, want)
		}
	}
	for _, bad := range []string{"", "portal.example.org", "ftp://x.example.org", "https://"} {
		if _, err := linkURL(bad); err == nil {
			t.Errorf("linkURL(%q) gave no error", bad)
		}
	}
}
