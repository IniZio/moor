// Command proxy is a task's proxy, the Box egress proxy or the desk proxy. It reads its task's policy from
// the portal and listens on port 8080.
package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/IniZio/moor/internal/proxy"
)

const systemRoots = "/etc/ssl/certs/ca-certificates.crt"

func main() {
	if err := run(); err != nil {
		slog.Error("proxy", "err", err)
		os.Exit(1)
	}
}

func run() error {
	cfg := proxy.Config{
		Portal: os.Getenv("MOOR_PORTAL"), Task: os.Getenv("MOOR_TASK"), Token: os.Getenv("MOOR_TOKEN"),
		Pass: os.Getenv("MOOR_PROXY_PASS"), Poll: time.Second, Log: slog.New(slog.NewTextHandler(os.Stderr, nil)),
	}
	if cfg.Portal == "" || cfg.Task == "" || cfg.Token == "" || cfg.Pass == "" {
		return errors.New("MOOR_PORTAL, MOOR_TASK, MOOR_TOKEN and MOOR_PROXY_PASS are required")
	}
	ca, cert, err := proxy.NewCA(time.Now())
	if err != nil {
		return fmt.Errorf("proxy: %w", err)
	}
	if err := proxy.WriteTrust(orDefault(os.Getenv("MOOR_CA_DIR"), "/ca"), cert, systemRoots); err != nil {
		return fmt.Errorf("proxy: %w", err)
	}
	cfg.CA = ca
	p := proxy.New(cfg)
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	go p.Run(ctx)
	srv := &http.Server{Addr: ":8080", Handler: p, ReadHeaderTimeout: 30 * time.Second}
	go func() {
		<-ctx.Done()
		_ = srv.Close()
	}()
	cfg.Log.Info("proxy listening", "addr", srv.Addr, "task", cfg.Task)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		return fmt.Errorf("serve: %w", err)
	}
	return nil
}

func orDefault(v, fallback string) string {
	if v == "" {
		return fallback
	}
	return v
}
