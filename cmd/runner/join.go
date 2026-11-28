package main

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/domain"
)

type joinOptions struct {
	options
	token, name string
	// enrolOnly joins and stops there. foreground runs the runner in this process, with no service.
	enrolOnly, foreground bool
}

func parseJoin(args []string) (joinOptions, error) {
	var o joinOptions
	host, _ := os.Hostname()
	fs := flag.NewFlagSet("runner join", flag.ContinueOnError)
	o.flags(fs, "the portal's address, such as https://portal.example.com")
	fs.StringVar(&o.token, "token", "", "the one-time join token the portal showed")
	fs.StringVar(&o.name, "name", host, "the machine's name in the portal")
	fs.BoolVar(&o.enrolOnly, "enrol-only", false, "join and stop there, without starting the runner")
	fs.BoolVar(&o.foreground, "foreground", false, "run the runner in this process after joining, without a systemd service")
	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("flags: %w", err)
	}
	if o.portal == "" || o.id == "" || o.token == "" || o.keyFile == "" {
		return o, errors.New("join needs -portal, -id, -token and -key")
	}
	im := o.images
	if !o.enrolOnly && (im.Box == "" || im.Desk == "" || im.Relay == "" || im.Proxy == "") {
		return o, errors.New("join starts the runner, so it needs -box-image, -desk-image, -relay-image and -proxy-image (before it uses the token); -enrol-only joins without starting it")
	}
	abs, err := filepath.Abs(o.keyFile)
	if err != nil {
		return o, fmt.Errorf("key: %w", err)
	}
	o.keyFile = abs
	return o, nil
}

// service keeps the runner running after the shell that joined it closes.
type service interface {
	Install(ctx context.Context, u unit) (installed, error)
}

// joinEnv is what join touches outside this process: its output, the user's systemd, the portal's link
// and the runner itself.
type joinEnv struct {
	out     io.Writer
	service service
	probe   func(ctx context.Context, cfg link.AgentConfig) error
	serve   func(ctx context.Context, o options) error
	exe     func() (string, error)
}

// hostEnv is this host: its systemd user manager if it has one, and this very binary.
func hostEnv() joinEnv {
	var svc service = noService{}
	if sd, err := systemd(); err == nil {
		svc = sd
	}
	return joinEnv{out: os.Stdout, service: svc, probe: dialProbe, serve: serve, exe: executable}
}

// noService is a host whose user systemd could not be found.
type noService struct{}

func (noService) Install(context.Context, unit) (installed, error) {
	return installed{}, errNoService
}

func executable() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("this binary's path: %w", err)
	}
	resolved, err := filepath.EvalSymlinks(exe)
	if err != nil {
		return "", fmt.Errorf("this binary's path: %w", err)
	}
	return resolved, nil
}

// dialProbe links once and lets go, to learn whether the portal takes this runner's key.
func dialProbe(ctx context.Context, cfg link.AgentConfig) error {
	c, _, err := link.Dial(ctx, cfg)
	if err != nil {
		return fmt.Errorf("probe: %w", err)
	}
	if err := c.Close(websocket.StatusNormalClosure, ""); err != nil {
		return fmt.Errorf("probe: %w", err)
	}
	return nil
}

// join enrols this host with the portal and then starts the runner, so one command does both. It is safe to
// run again: the key is made once, a join the portal refuses as used is taken as done when the portal knows
// this key, and the service converges on the newest flags.
func join(ctx context.Context, args []string, env joinEnv) error {
	o, err := parseJoin(args)
	if err != nil {
		return err
	}
	linkAt, err := linkURL(o.portal)
	if err != nil {
		return err
	}
	key, err := keyFor(o.keyFile)
	if err != nil {
		return err
	}
	if err := enrol(ctx, o, linkAt, key, env); err != nil {
		return err
	}
	if o.enrolOnly {
		return say(env.out, "Joined as %s. Nothing is started: run the runner with -portal %s -id %s -key %s and the image flags.\n", o.id, linkAt, o.id, o.keyFile)
	}
	o.portal = linkAt
	return start(ctx, o, env)
}

func say(w io.Writer, format string, args ...any) error {
	if _, err := fmt.Fprintf(w, format, args...); err != nil {
		return fmt.Errorf("output: %w", err)
	}
	return nil
}

// linkURL is where the runner dials for the portal at base.
func linkURL(base string) (string, error) {
	u, err := url.Parse(strings.TrimSuffix(base, "/"))
	if err != nil || u.Host == "" {
		return "", fmt.Errorf("%q is not the portal's address, such as https://portal.example.com", base)
	}
	switch u.Scheme {
	case "https":
		u.Scheme = "wss"
	case "http":
		u.Scheme = "ws"
	default:
		return "", fmt.Errorf("%q must start with https:// or http://", base)
	}
	u.Path = strings.TrimSuffix(u.Path, "/") + "/runner/link"
	return u.String(), nil
}

// enrol sends the join. A token the portal says is used may be this very join sent twice, so then the
// portal's answer to this key decides.
func enrol(ctx context.Context, o joinOptions, linkAt string, key ed25519.PrivateKey, env joinEnv) error {
	err := postJoin(ctx, o, key)
	var refused *refusedError
	cfg := link.AgentConfig{URL: linkAt, Runner: domain.RunnerID(o.id), Key: key, Build: version}
	switch {
	case err == nil:
		return say(env.out, "Joined as %s. The key is in %s.\n", o.id, o.keyFile)
	case !errors.As(err, &refused) || refused.status != http.StatusForbidden || env.probe(ctx, cfg) != nil:
		return err
	}
	return say(env.out, "%s is already enrolled with the key in %s.\n", o.id, o.keyFile)
}

// refusedError is a join the portal answered with a status other than 200.
type refusedError struct {
	status int
	text   string
}

func (e *refusedError) Error() string {
	return fmt.Sprintf("join refused (%d %s): %s", e.status, http.StatusText(e.status), e.text)
}

func postJoin(ctx context.Context, o joinOptions, key ed25519.PrivateKey) error {
	body, err := json.Marshal(map[string]any{"id": o.id, "token": o.token, "name": o.name, "public_key": []byte(key.Public().(ed25519.PublicKey))})
	if err != nil {
		return fmt.Errorf("join: %w", err)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, strings.TrimSuffix(o.portal, "/")+"/runner/join", bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("join: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := (&http.Client{Timeout: 30 * time.Second}).Do(req)
	if err != nil {
		return fmt.Errorf("join: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		msg, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<10))
		return &refusedError{status: resp.StatusCode, text: strings.TrimSpace(string(msg))}
	}
	return nil
}

// start runs the runner so that it outlives the shell: as a systemd user service when this host has a user
// manager, which is the smallest mature supervisor there is. Without one it runs in this shell, and says so.
func start(ctx context.Context, o joinOptions, env joinEnv) error {
	if o.foreground {
		return runHere(ctx, o, env, "Running the runner in this shell.")
	}
	exe, err := env.exe()
	if err != nil {
		return err
	}
	u, err := unitFor(exe, o.options)
	if err != nil {
		return err
	}
	got, err := env.service.Install(ctx, u)
	switch {
	case errors.Is(err, errNoService):
		return runHere(ctx, o, env, "This host has no systemd user manager.")
	case err != nil:
		return fmt.Errorf("start the runner: %w", err)
	}
	stay := "It keeps running after this shell closes and starts again after a reboot."
	if !got.Persistent {
		stay = "It keeps running after this shell closes while you stay logged in. Run loginctl enable-linger to keep it after you log out."
	}
	return say(env.out, "Started %s as a systemd user service (%s).\n%s\nLogs: journalctl --user -u %s\nStop and remove it: systemctl --user disable --now %s\n", u.name, got.Path, stay, u.name, u.name)
}

func runHere(ctx context.Context, o joinOptions, env joinEnv, why string) error {
	if err := say(env.out, "%s The runner stops when this shell closes.\n", why); err != nil {
		return err
	}
	return env.serve(ctx, o.options)
}

// keyFor reads the key in path, or makes one and writes it there, readable by this user alone. A
// join run again after a failure keeps the key it already made.
func keyFor(path string) (ed25519.PrivateKey, error) {
	key, err := readKey(path)
	if err == nil || !errors.Is(err, fs.ErrNotExist) {
		return key, err
	}
	seed := make([]byte, ed25519.SeedSize)
	if _, err := rand.Read(seed); err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	_, err = f.WriteString(base64.StdEncoding.EncodeToString(seed) + "\n")
	if err = errors.Join(err, f.Close()); err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	return ed25519.NewKeyFromSeed(seed), nil
}
