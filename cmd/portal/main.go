// Command portal serves the product: pages, webhooks, the product MCP and runner links.
package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"
)

var version = "dev"

const shutdownGrace = 10 * time.Second

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := run(ctx, os.Args[1:], os.Getenv); err != nil {
		fmt.Fprintln(os.Stderr, "portal:", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, args []string, getenv func(string) string) error {
	cfg, err := parseConfig(args, getenv)
	if err != nil {
		return err
	}
	handler, closeApp, err := build(ctx, cfg)
	if err != nil {
		return err
	}
	defer closeApp()
	ln, err := net.Listen("tcp", cfg.listen)
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}
	streams, endStreams := context.WithCancel(context.Background())
	defer endStreams()
	srv := &http.Server{
		Handler:           handler,
		BaseContext:       func(net.Listener) context.Context { return streams },
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       30 * time.Second,
		WriteTimeout:      30 * time.Second,
		IdleTimeout:       2 * time.Minute,
	}
	srv.RegisterOnShutdown(endStreams)
	served := make(chan error, 1)
	go func() { served <- srv.Serve(ln) }()
	log.Printf("portal %s listening on %s", version, ln.Addr())
	select {
	case err := <-served:
		return fmt.Errorf("serve: %w", err)
	case <-ctx.Done():
	}
	log.Print("portal shutting down")
	shutCtx, cancel := context.WithTimeout(context.Background(), shutdownGrace)
	defer cancel()
	if err := srv.Shutdown(shutCtx); err != nil && !errors.Is(err, http.ErrServerClosed) {
		return fmt.Errorf("shutdown: %w", err)
	}
	return nil
}
