// Command runner runs tasks on a host and dials out to the portal.
package main

import (
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/IniZio/moor/internal/adapter/docker"
	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/runner"
)

var version = "dev"

func main() {
	if err := run(os.Args[1:]); err != nil {
		slog.Error("runner", "err", err)
		os.Exit(1)
	}
}

type options struct {
	portal, id, keyFile, prefix string
	images                      docker.Images
	slots                       int
}

func parse(args []string) (options, error) {
	var o options
	fs := flag.NewFlagSet("runner", flag.ContinueOnError)
	fs.StringVar(&o.portal, "portal", "", "the portal's link URL, such as wss://portal.example.com/runner/link")
	fs.StringVar(&o.id, "id", "", "this runner's id, as the portal enrolled it")
	fs.StringVar(&o.keyFile, "key", "", "file holding this runner's ed25519 seed, base64")
	fs.StringVar(&o.prefix, "prefix", "moor", "prefix of every Docker object the runner makes")
	fs.StringVar(&o.images.Box, "box-image", "", "Box image")
	fs.StringVar(&o.images.Desk, "desk-image", "", "desk image")
	fs.StringVar(&o.images.Relay, "relay-image", "", "relay image")
	fs.StringVar(&o.images.Proxy, "proxy-image", "", "proxy image")
	fs.IntVar(&o.slots, "slots", 1, "tasks this machine runs at once")
	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("flags: %w", err)
	}
	if o.portal == "" || o.id == "" || o.keyFile == "" {
		return o, errors.New("-portal, -id and -key are required")
	}
	return o, nil
}

func run(args []string) error {
	if len(args) > 0 && args[0] == "join" {
		return join(args[1:])
	}
	o, err := parse(args)
	if err != nil {
		return err
	}
	key, err := readKey(o.keyFile)
	if err != nil {
		return err
	}
	d, err := docker.New(docker.Config{Prefix: o.prefix, Images: o.images})
	if err != nil {
		return fmt.Errorf("docker: %w", err)
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	agent := link.AgentConfig{URL: o.portal, Runner: domain.RunnerID(o.id), Key: key, Build: version}
	r := newRunner(agent, host{d}, sessions{connect: host{d}.connect, attach: host{d}.attach, argv: []string{"harness"}})
	if err := r.Run(ctx); err != nil {
		return fmt.Errorf("run: %w", err)
	}
	return nil
}

// newRunner is where the adapters meet the runner.
func newRunner(agent link.AgentConfig, h runner.Host, s runner.Sessions) *runner.Runner {
	return runner.New(runner.Config{
		Dial: dialer(agent), Host: h, Sessions: s,
		Build: agent.Build, Machine: string(agent.Runner), Slots: 1, Log: slog.New(slog.NewTextHandler(os.Stderr, nil)),
	})
}

func readKey(path string) (ed25519.PrivateKey, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	defer func() { _ = f.Close() }()
	b, err := io.ReadAll(io.LimitReader(f, 1<<10))
	if err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	seed, err := base64.StdEncoding.DecodeString(strings.TrimSpace(string(b)))
	if err != nil || len(seed) != ed25519.SeedSize {
		return nil, fmt.Errorf("key: %s does not hold a base64 ed25519 seed", path)
	}
	return ed25519.NewKeyFromSeed(seed), nil
}
