package e2e

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"
)

const (
	dockerEnv  = "MOOR_TEST_DOCKER"
	userHeader = "X-E2E-User"
	secretKey  = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
)

func randHex(t *testing.T, n int) string {
	t.Helper()
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	return hex.EncodeToString(b)
}

// build compiles a command of this module into dir.
func build(t *testing.T, dir, pkg string) string {
	t.Helper()
	out := filepath.Join(dir, filepath.Base(pkg))
	if b, err := exec.Command("go", "build", "-o", out, "../"+pkg).CombinedOutput(); err != nil {
		t.Fatalf("build %s: %v: %s", pkg, err, b)
	}
	return out
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

// proc is a started process whose output goes to a file the test prints when it fails.
type proc struct {
	cmd *exec.Cmd
	log string
}

func start(t *testing.T, name string, env []string, bin string, args ...string) *proc {
	t.Helper()
	logPath := filepath.Join(t.TempDir(), name+".log")
	f, err := os.Create(logPath)
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(bin, args...)
	cmd.Env, cmd.Stdout, cmd.Stderr = append(os.Environ(), env...), f, f
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	p := &proc{cmd: cmd, log: logPath}
	t.Cleanup(func() {
		p.stop(t)
		_ = f.Close()
		if t.Failed() {
			b, _ := os.ReadFile(logPath)
			t.Logf("%s log:\n%s", name, tail(string(b), 60))
		}
	})
	return p
}

// stop sends SIGTERM and waits for the process, as a deploy stops a pod.
func (p *proc) stop(t *testing.T) {
	t.Helper()
	if p.cmd.ProcessState != nil {
		return
	}
	_ = p.cmd.Process.Signal(syscall.SIGTERM)
	done := make(chan struct{})
	go func() { _ = p.cmd.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(30 * time.Second):
		_ = p.cmd.Process.Kill()
		<-done
		t.Errorf("%s did not stop within 30s of SIGTERM", p.cmd.Path)
	}
}

func tail(s string, n int) string {
	lines := strings.Split(s, "\n")
	return strings.Join(lines[max(0, len(lines)-n):], "\n")
}

// portal is a portal process on a fixed address, so a restart keeps the address runners dial.
type portal struct {
	bin, addr, dsn string
	p              *proc
}

func (pt *portal) start(t *testing.T) {
	t.Helper()
	env := []string{
		"PORTAL_LISTEN=" + pt.addr, "PORTAL_DATABASE_URL=" + pt.dsn, "PORTAL_USER_HEADER=" + userHeader,
		"PORTAL_SECRET_KEY=" + secretKey, "PORTAL_PUBLIC_URL=http://" + pt.addr,
	}
	pt.p = start(t, "portal", env, pt.bin)
	eventually(t, time.Minute, "the portal answers", func() bool {
		resp, err := http.Get("http://" + pt.addr + "/healthz")
		if err == nil {
			_ = resp.Body.Close()
		}
		return err == nil && resp.StatusCode == http.StatusOK
	})
}

// call sends a request as the signed-in person Mia and decodes a JSON answer into out.
func (pt *portal) call(t *testing.T, method, path string, body any, out any) {
	t.Helper()
	var r io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		r = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, "http://"+pt.addr+path, r)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set(userHeader, "Mia")
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(resp.Body)
	if resp.StatusCode >= 300 {
		t.Fatalf("%s %s: %s: %s", method, path, resp.Status, b)
	}
	if out != nil {
		if err := json.Unmarshal(b, out); err != nil {
			t.Fatalf("%s %s: %v in %s", method, path, err, b)
		}
	}
}

func eventually(t *testing.T, limit time.Duration, what string, ok func() bool) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), limit)
	defer cancel()
	for !ok() {
		select {
		case <-ctx.Done():
			t.Fatalf("timed out after %s waiting for %s", limit, what)
		case <-time.After(250 * time.Millisecond):
		}
	}
}

func docker(t *testing.T, args ...string) []string {
	t.Helper()
	out, err := exec.Command("docker", args...).CombinedOutput()
	if err != nil {
		t.Fatalf("docker %v: %v: %s", args, err, out)
	}
	f := strings.Fields(string(out))
	slices.Sort(f)
	return f
}

func running(t *testing.T, prefix string) []string {
	t.Helper()
	return docker(t, "ps", "-q", "--no-trunc", "--filter", "label=moor.product="+prefix)
}

var kinds = []string{"container", "network", "volume"}

// remaining lists what the runner of prefix has on the host, by kind.
func remaining(t *testing.T, prefix string) map[string][]string {
	t.Helper()
	ls := map[string][]string{
		"container": {"ps", "-a", "-q", "--filter", "name=" + prefix},
		"network":   {"network", "ls", "-q", "--filter", "name=" + prefix},
		"volume":    {"volume", "ls", "-q", "--filter", "name=" + prefix},
	}
	left := map[string][]string{}
	for _, kind := range kinds {
		if ids := docker(t, ls[kind]...); len(ids) > 0 {
			left[kind] = ids
		}
	}
	return left
}

// leftBehind is what remains of prefix, removed so a failed run leaves nothing either.
func leftBehind(t *testing.T, prefix string) map[string][]string {
	t.Helper()
	left := remaining(t, prefix)
	rm := map[string][]string{"container": {"rm", "-f"}, "network": {"network", "rm"}, "volume": {"volume", "rm", "-f"}}
	for _, kind := range kinds {
		if ids := left[kind]; len(ids) > 0 {
			_, _ = exec.Command("docker", append(rm[kind], ids...)...).CombinedOutput()
		}
	}
	return left
}

func runCmd(name string, args ...string) (string, error) {
	out, err := exec.Command(name, args...).CombinedOutput()
	if err != nil {
		return string(out), fmt.Errorf("%s: %w", name, err)
	}
	return string(out), nil
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(fmt.Errorf("e2e: %w", err))
	}
}
