package docker

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

const dockerEnv = "MOOR_TEST_DOCKER"

// testImages are the images make images built, under the tag make passes in MOOR_IMAGE_TAG.
var testImages = Images{Box: image("box"), Desk: image("desk"), Relay: image("relay"), Proxy: image("proxy")}

func image(name string) string {
	tag := os.Getenv("MOOR_IMAGE_TAG")
	if tag == "" {
		tag = "dev"
	}
	return "moor-fresh-" + name + ":" + tag
}

// boxAllow is a public package registry plus the Docker Hub hosts a nested pull needs.
var boxAllow = []string{
	"registry.npmjs.org", "registry-1.docker.io", "auth.docker.io",
	"production.cloudflare.docker.com", "production.cloudfront.docker.com",
	"docker-images-prod.6aa30f8b08e16409b46e0173d6de2f56.r2.cloudflarestorage.com",
}

const nestedImage = "curlimages/curl:8.17.0@sha256:935d9100e9ba842cdb060de42472c7ca90cfe9a7c96e4dacb55e79e560b3ff40"

// fakePortal serves the proxies their policy from the host, at the Docker bridge's gateway, which a task's
// egress network reaches. The Box egress proxy gets boxAllow and the desk proxy one registry.
func fakePortal(t *testing.T) string {
	t.Helper()
	gw := sh(t, "docker", "network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}")
	ln, err := net.Listen("tcp", net.JoinHostPort(gw, "0"))
	if err != nil {
		t.Fatal(err)
	}
	rule := func(hosts ...string) []any {
		return []any{map[string]any{"name": "registry", "hosts": hosts, "allow": []string{"*"}}}
	}
	policies := map[string]any{
		"Bearer box.t":  map[string]any{"version": 1, "rules": rule(boxAllow...)},
		"Bearer desk.t": map[string]any{"version": 1, "rules": rule("registry.npmjs.org")},
	}
	srv := &http.Server{ReadHeaderTimeout: 5 * time.Second, Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p, ok := policies[r.Header.Get("Authorization")]
		switch {
		case !ok:
			http.Error(w, "no", http.StatusUnauthorized)
		case r.Method == http.MethodGet && r.URL.Path == "/internal/policy/t1":
			_ = json.NewEncoder(w).Encode(p)
		default:
			w.WriteHeader(http.StatusNoContent)
		}
	})}
	go func() { _ = srv.Serve(ln) }()
	t.Cleanup(func() { _ = srv.Close() })
	return "http://" + ln.Addr().String()
}

// adapter gives the test its own prefix and fails it if anything with that prefix is left after it.
// This is the only skip in the package: without Docker and the images there is nothing to run.
func adapter(t *testing.T) Config {
	t.Helper()
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: these tests start real containers, run them with make test-docker", dockerEnv)
	}
	b := make([]byte, 4)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	cfg := Config{Prefix: "moorft-" + hex.EncodeToString(b), Images: testImages}
	t.Cleanup(func() { assertNothingLeft(t, cfg.Prefix) })
	return cfg
}

func assertNothingLeft(t *testing.T, prefix string) {
	t.Helper()
	for _, kind := range []string{"container", "network", "volume"} {
		args := []string{kind, "ls", "-q", "--filter", "name=" + prefix}
		if kind == "container" {
			args = []string{"ps", "-a", "-q", "--filter", "name=" + prefix}
		}
		left := strings.Fields(sh(t, "docker", args...))
		if len(left) > 0 {
			t.Errorf("%d %s(s) named %s* left behind: %v", len(left), kind, prefix, left)
			rm := map[string][]string{"container": {"rm", "-f"}, "network": {"network", "rm"}, "volume": {"volume", "rm", "-f"}}[kind]
			_, _ = exec.Command("docker", append(rm, left...)...).CombinedOutput()
		}
	}
}

func sh(t *testing.T, name string, args ...string) string {
	t.Helper()
	out, err := exec.Command(name, args...).CombinedOutput()
	if err != nil {
		t.Fatalf("%s %v: %v: %s", name, args, err, out)
	}
	return strings.TrimSpace(string(out))
}

// inBox runs a command the way the desk's harness does: bash -c in the desk, carried into the Box.
func inBox(d *Docker, task domain.TaskID, script string) (string, error) {
	out, err := exec.Command("docker", "exec", "-e", "BOX_SSH=rootless@"+d.name(task, string(Box)),
		d.name(task, string(Desk)), "bash", "-c", script).CombinedOutput()
	return strings.TrimSpace(string(out)), err
}

func ids(t *testing.T, d *Docker) map[Part]string {
	t.Helper()
	tasks, err := d.List(context.Background())
	if err != nil || len(tasks) != 1 {
		t.Fatalf("list: %v, %d tasks", err, len(tasks))
	}
	got := map[Part]string{}
	for _, c := range tasks[0].Containers {
		if !c.Running {
			t.Errorf("%s is %s", c.Name, c.State)
		}
		got[c.Part] = sh(t, "docker", "inspect", "-f", "{{.Id}}", c.Name)
	}
	return got
}

func waitFile(t *testing.T, container, path string) string {
	t.Helper()
	for range 240 {
		out, err := exec.Command("docker", "exec", container, "cat", path).CombinedOutput()
		if err == nil {
			return strings.TrimSpace(string(out))
		}
		time.Sleep(500 * time.Millisecond)
	}
	t.Fatalf("%s never appeared in %s", path, container)
	return ""
}

func TestTaskLifecycle(t *testing.T) {
	cfg := adapter(t)
	d, err := New(cfg)
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	key := Key{Task: "t1", Run: 1, Gen: 1}
	spec := Spec{Key: key, Portal: fakePortal(t), BoxToken: "box.t", DeskToken: "desk.t", TaskToken: "task.t"}
	t.Cleanup(func() { _ = d.Down(context.Background(), Key{Task: key.Task, Run: 99, Gen: 99}) })

	start := time.Now()
	if err := d.Up(ctx, spec); err != nil {
		t.Fatalf("up: %v", err)
	}
	t.Logf("up to ready in %s", time.Since(start).Round(100*time.Millisecond))
	first := ids(t, d)
	if len(first) != len(parts) {
		t.Fatalf("want %d parts running, got %v", len(parts), first)
	}

	t.Run("only the desk proxy holds the task token", func(t *testing.T) { onlyTheDeskProxyHoldsTheTaskToken(t, d, key.Task) })
	t.Run("box cannot reach the desk proxy", func(t *testing.T) { boxCannotReachDeskProxy(t, d, key.Task) })
	t.Run("stand-in runs three commands in the box", func(t *testing.T) { standIn(t, d, key.Task) })
	t.Run("nested container fetches and writes a bind mount", func(t *testing.T) { nested(t, d, key.Task) })
	t.Run("up twice changes nothing", func(t *testing.T) {
		if err := d.Up(ctx, spec); err != nil {
			t.Fatal(err)
		}
		if again := ids(t, d); !equal(first, again) {
			t.Fatalf("containers changed: %v then %v", first, again)
		}
	})
	t.Run("recreate replaces only the box", func(t *testing.T) { recreate(t, d, spec, first) })
	t.Run("another adapter lists the task and takes it down", func(t *testing.T) { readopt(t, cfg, Key{Task: key.Task, Run: 1, Gen: 2}) })
}

// onlyTheDeskProxyHoldsTheTaskToken checks the desk holds the placeholder its publish tool presents, and the
// desk proxy, which swaps it on the wire, holds the token.
func onlyTheDeskProxyHoldsTheTaskToken(t *testing.T, d *Docker, task domain.TaskID) {
	for _, p := range parts {
		env := sh(t, "docker", "inspect", "-f", "{{range .Config.Env}}{{println .}}{{end}}", d.name(task, string(p)))
		if has := strings.Contains(env, "task.t"); has != (p == DeskProxy) {
			t.Errorf("%s holds the task token: %v", p, has)
		}
		if p == Desk && (!strings.Contains(env, "MOOR_TASK=t1\n") || !strings.Contains(env, "MOOR_TASK_TOKEN="+taskPlaceholder+"\n")) {
			t.Errorf("the desk does not know its task or lacks the placeholder:\n%s", env)
		}
	}
}

func equal(a, b map[Part]string) bool {
	if len(a) != len(b) {
		return false
	}
	for k, v := range a {
		if b[k] != v {
			return false
		}
	}
	return true
}

func boxCannotReachDeskProxy(t *testing.T, d *Docker, task domain.TaskID) {
	ipsOut := sh(t, "docker", "inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}", d.name(task, string(DeskProxy)))
	ips := strings.Fields(ipsOut)
	if len(ips) != 2 {
		t.Fatalf("desk proxy addresses: %q", ipsOut)
	}
	if out, err := exec.Command("docker", "exec", d.name(task, string(Desk)), "bash.real", "-c",
		"exec 3<>/dev/tcp/"+d.name(task, string(DeskProxy))+"/8080").CombinedOutput(); err != nil {
		t.Fatalf("the desk cannot reach its own proxy, so this check proves nothing: %v: %s", err, out)
	}
	for _, target := range append(ips, d.name(task, string(DeskProxy))) {
		out, err := exec.Command("docker", "exec", d.name(task, string(Box)),
			"curl", "-sS", "-m", "3", "--noproxy", "*", "http://"+target+":8080/").CombinedOutput()
		if err == nil {
			t.Errorf("box reached the desk proxy at %s: %s", target, out)
		}
	}
}

func standIn(t *testing.T, d *Docker, task domain.TaskID) {
	desk := d.name(task, string(Desk))
	sh(t, "docker", "exec", desk, "hc", "run", "--bg", "--name", "task", "--no-parent", "--cwd", "/workspace", "harness")
	sh(t, "docker", "exec", desk, "touch", "/desk/go")
	got := waitFile(t, desk, "/desk/standin.out")
	lines := strings.Split(got, "\n")
	ok := len(lines) == 3 && lines[0] == "write rc=0 0" && lines[1] == "allowed rc=0 200" &&
		strings.HasPrefix(lines[2], "blocked rc=") && !strings.HasPrefix(lines[2], "blocked rc=0 ") && strings.HasSuffix(lines[2], " 403")
	if !ok {
		t.Fatalf("stand-in wrote %q, want write as uid 0 in the namespace, 200 from the allowed host, a failed fetch with 403 from the blocked one", lines)
	}
	if owner := sh(t, "docker", "exec", d.name(task, string(Box)), "stat", "-c", "%u", "/workspace/standin.txt"); owner != "1000" {
		t.Fatalf("the file the stand-in wrote is owned by %s outside the namespace, want the Box user 1000", owner)
	}
}

func nested(t *testing.T, d *Docker, task domain.TaskID) {
	out, err := inBox(d, task, `mkdir -p /workspace/nested && docker run --rm -u "$(id -u):$(id -g)" -v /workspace/nested:/out `+
		nestedImage+` -s -o /out/page -w '%{http_code}' https://registry.npmjs.org/`)
	if err != nil || !strings.HasSuffix(out, "200") {
		t.Fatalf("nested fetch: %v: %s", err, out)
	}
	if owner := sh(t, "docker", "exec", d.name(task, string(Box)), "stat", "-c", "%u", "/workspace/nested/page"); owner != "1000" {
		t.Fatalf("the nested container's file is owned by %s outside the namespace, want 1000", owner)
	}
}

func recreate(t *testing.T, d *Docker, spec Spec, before map[Part]string) {
	ctx := context.Background()
	desk := d.name(spec.Key.Task, string(Desk))
	sh(t, "docker", "exec", desk, "sh", "-c", "echo kept > /desk/marker")
	next := spec
	next.Key.Gen++
	if err := d.Recreate(ctx, next); err != nil {
		t.Fatalf("recreate: %v", err)
	}
	after := ids(t, d)
	for _, p := range parts {
		if changed := after[p] != before[p]; changed != (p == Box) {
			t.Errorf("%s: replaced %v, want replaced only for the box", p, changed)
		}
	}
	if got := sh(t, "docker", "exec", desk, "cat", "/desk/marker"); got != "kept" {
		t.Fatalf("desk volume after recreate: %q", got)
	}
	if err := d.Up(ctx, spec); !errors.Is(err, ErrStale) {
		t.Fatalf("up with the older key: %v, want ErrStale", err)
	}
}

func readopt(t *testing.T, cfg Config, key Key) {
	ctx := context.Background()
	d, err := New(cfg)
	if err != nil {
		t.Fatal(err)
	}
	tasks, err := d.List(ctx)
	if err != nil || len(tasks) != 1 || tasks[0].Key != key || len(tasks[0].Containers) != len(parts) {
		t.Fatalf("list after restart: %v, %+v, want one task with key %+v and %d containers", err, tasks, key, len(parts))
	}
	if err := d.Down(ctx, key); err != nil {
		t.Fatalf("down: %v", err)
	}
	if err := d.Down(ctx, key); err != nil {
		t.Fatalf("second down: %v", err)
	}
	if tasks, err := d.List(ctx); err != nil || len(tasks) != 0 {
		t.Fatalf("list after down: %v, %+v", err, tasks)
	}
	assertNothingLeft(t, cfg.Prefix)
}
