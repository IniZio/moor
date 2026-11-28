package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/IniZio/moor/internal/runner"
)

// errNoService means this host has no systemd user manager to keep the runner running.
var errNoService = errors.New("this host has no systemd user manager")

// unit is a systemd user service that runs argv.
type unit struct {
	name, description string
	argv              []string
}

var idRE = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}$`)

// unitFor is the service that runs the runner at exe with o's flags. Its name is made of the machine id, so one
// host can serve more than one portal, and the id may not leave the unit folder.
func unitFor(exe string, o options) (unit, error) {
	if !idRE.MatchString(o.id) {
		return unit{}, fmt.Errorf("machine id %q cannot name a service", o.id)
	}
	argv := []string{
		exe, "-portal", o.portal, "-id", o.id, "-key", o.keyFile, "-prefix", o.prefix,
		"-box-image", o.images.Box, "-desk-image", o.images.Desk, "-relay-image", o.images.Relay, "-proxy-image", o.images.Proxy,
	}
	if o.cloneURL != "" && o.cloneURL != runner.DefaultCloneURL {
		argv = append(argv, "-clone-url", o.cloneURL)
	}
	if o.harness != "" {
		argv = append(argv, "-harness", o.harness)
	}
	return unit{name: "moor-runner-" + o.id + ".service", description: "moor runner " + o.id, argv: argv}, nil
}

// quoter writes an argument as one word of an ExecStart line: systemd reads backslash and double quote inside
// double quotes, and expands a percent sign and a dollar sign.
var quoter = strings.NewReplacer(`\`, `\\`, `"`, `\"`, "%", "%%", "$", "$$", "\n", `\n`)

func (u unit) text() string {
	words := make([]string, len(u.argv))
	for i, a := range u.argv {
		words[i] = `"` + quoter.Replace(a) + `"`
	}
	return "[Unit]\nDescription=" + u.description + "\n\n[Service]\nExecStart=" + strings.Join(words, " ") +
		"\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n"
}

// installed is where a service went and whether it outlives this login.
type installed struct {
	Path       string
	Persistent bool
}

// userService keeps units in dir and drives the user's systemd through ctl. linger lets the user's units run
// without a login session.
type userService struct {
	dir    string
	ctl    func(ctx context.Context, args ...string) error
	linger func(ctx context.Context) error
}

// Install writes the unit, reloads systemd, enables the unit and restarts it, so a repeat with the same or a
// changed unit ends in the same state: running the newest unit. A host with no user manager gets errNoService
// and no file.
func (s userService) Install(ctx context.Context, u unit) (installed, error) {
	if err := s.ctl(ctx, "show-environment"); err != nil {
		return installed{}, fmt.Errorf("%w: %w", errNoService, err)
	}
	if err := os.MkdirAll(s.dir, 0o755); err != nil {
		return installed{}, fmt.Errorf("unit folder: %w", err)
	}
	path := filepath.Join(s.dir, u.name)
	if err := os.WriteFile(path, []byte(u.text()), 0o644); err != nil {
		return installed{}, fmt.Errorf("write %s: %w", path, err)
	}
	for _, args := range [][]string{{"daemon-reload"}, {"enable", u.name}, {"restart", u.name}} {
		if err := s.ctl(ctx, args...); err != nil {
			return installed{}, err
		}
	}
	return installed{Path: path, Persistent: s.linger(ctx) == nil}, nil
}

// systemd is the user's own systemd, as the systemctl and loginctl commands reach it.
func systemd() (userService, error) {
	conf, err := os.UserConfigDir()
	if err != nil {
		return userService{}, fmt.Errorf("user config folder: %w", err)
	}
	return userService{dir: filepath.Join(conf, "systemd", "user"), ctl: systemctl, linger: lingerOn}, nil
}

func systemctl(ctx context.Context, args ...string) error {
	out, err := exec.CommandContext(ctx, "systemctl", append([]string{"--user"}, args...)...).CombinedOutput()
	if err != nil {
		return fmt.Errorf("systemctl --user %s: %w: %s", strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	}
	return nil
}

// lingerOn asks for the user's units to keep running after the last login ends.
func lingerOn(ctx context.Context) error {
	if out, err := exec.CommandContext(ctx, "loginctl", "enable-linger").CombinedOutput(); err != nil {
		return fmt.Errorf("loginctl enable-linger: %w: %s", err, strings.TrimSpace(string(out)))
	}
	return nil
}
