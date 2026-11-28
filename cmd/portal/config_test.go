package main

import (
	"context"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store/pgtest"
)

func env(m map[string]string) func(string) string { return func(k string) string { return m[k] } }

func goodEnv() map[string]string {
	return map[string]string{"PORTAL_DATABASE_URL": "postgres://x", "PORTAL_USER_HEADER": "X-Who", "PORTAL_SECRET_KEY": testKeyB64}
}

func TestConfigReadsEnvironmentAndFlagsOverrideIt(t *testing.T) {
	e := goodEnv()
	e["PORTAL_LISTEN"] = ":9000"
	e["PORTAL_TRUSTED_ORIGINS"] = "https://a.example, https://b.example,"
	c, err := parseConfig(nil, env(e))
	if err != nil || c.listen != ":9000" || c.userHeader != "X-Who" || len(c.trustedOrigins) != 2 || c.trustedOrigins[1] != "https://b.example" {
		t.Fatalf("config = %+v, %v", c, err)
	}
	c, err = parseConfig([]string{"-listen", ":9100", "-user-header", "X-Other", "-org-name", "example-org"}, env(e))
	if err != nil || c.listen != ":9100" || c.userHeader != "X-Other" || c.orgName != "example-org" {
		t.Fatalf("config with flags = %+v, %v", c, err)
	}
	if c, _ := parseConfig(nil, env(goodEnv())); c.listen != "127.0.0.1:8080" {
		t.Errorf("default listen address = %q", c.listen)
	}
}

func TestConfigRefusesWhatIsMissingOrWrong(t *testing.T) {
	for name, drop := range map[string]string{"user header": "PORTAL_USER_HEADER", "database": "PORTAL_DATABASE_URL", "key": "PORTAL_SECRET_KEY"} {
		e := goodEnv()
		delete(e, drop)
		if _, err := parseConfig(nil, env(e)); err == nil {
			t.Errorf("a config without the %s was accepted", name)
		}
	}
	short := goodEnv()
	short["PORTAL_SECRET_KEY"] = "AAEC"
	if _, err := parseConfig(nil, env(short)); err == nil || strings.Contains(err.Error(), "AAEC") {
		t.Errorf("a short key: %v", err)
	}
	if _, err := parseConfig([]string{"-nope"}, env(goodEnv())); err == nil {
		t.Error("an unknown flag was accepted")
	}
}

func freeAddr(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = ln.Close() }()
	return ln.Addr().String()
}

func waitReady(t *testing.T, addr string) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		resp, err := http.Get("http://" + addr + "/readyz")
		if err == nil {
			_ = resp.Body.Close()
			if resp.StatusCode == http.StatusOK {
				return
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("the server never became ready: %v", err)
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func TestTheServerAnswersHealthChecksAndStopsGracefullyOnCancel(t *testing.T) {
	addr := freeAddr(t)
	e := goodEnv()
	e["PORTAL_DATABASE_URL"] = pgtest.SchemaDSN(t)
	e["PORTAL_LISTEN"] = addr
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- run(ctx, nil, env(e)) }()
	waitReady(t, addr)
	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Errorf("run returned %v after cancel", err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the server did not stop after cancel")
	}
	if _, err := http.Get("http://" + addr + "/healthz"); err == nil {
		t.Error("the server still answers after it stopped")
	}
}
