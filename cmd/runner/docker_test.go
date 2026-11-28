package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"os"
	"os/exec"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/docker"
	"github.com/IniZio/moor/internal/runner"
	"github.com/IniZio/moor/internal/wire"
)

const dockerEnv = "MOOR_TEST_DOCKER"

// testImages are the images make images built, under the tag make passes in MOOR_IMAGE_TAG.
var testImages = docker.Images{Box: image("box"), Desk: image("desk"), Relay: image("relay"), Proxy: image("proxy")}

func image(name string) string {
	tag := os.Getenv("MOOR_IMAGE_TAG")
	if tag == "" {
		tag = "dev"
	}
	return "moor-fresh-" + name + ":" + tag
}

// realHost gives the test a Docker adapter of its own prefix, and fails the test if anything of that
// prefix is left after it, removing it so a failed run leaves nothing either.
func realHost(t *testing.T) (host, string) {
	t.Helper()
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	b := make([]byte, 4)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	prefix := "moorrt-" + hex.EncodeToString(b)
	d, err := docker.New(docker.Config{Prefix: prefix, Images: testImages})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = d.Down(context.Background(), docker.Key{Task: "t1", Run: 1 << 40, Gen: 1 << 40})
		nothingLeft(t, prefix)
	})
	return host{d}, prefix
}

func nothingLeft(t *testing.T, prefix string) {
	t.Helper()
	ls := map[string][]string{
		"container": {"ps", "-a", "-q", "--filter", "name=" + prefix},
		"network":   {"network", "ls", "-q", "--filter", "name=" + prefix},
		"volume":    {"volume", "ls", "-q", "--filter", "name=" + prefix},
	}
	rm := map[string][]string{"container": {"rm", "-f"}, "network": {"network", "rm"}, "volume": {"volume", "rm", "-f"}}
	for _, kind := range []string{"container", "network", "volume"} {
		if left := dockerLines(t, ls[kind]...); len(left) > 0 {
			t.Errorf("%d %s(s) named %s* left behind", len(left), kind, prefix)
			_, _ = exec.Command("docker", append(rm[kind], left...)...).CombinedOutput()
		}
	}
}

func dockerLines(t *testing.T, args ...string) []string {
	t.Helper()
	out, err := exec.Command("docker", args...).CombinedOutput()
	if err != nil {
		t.Fatalf("docker %v: %v: %s", args, err, out)
	}
	f := strings.Fields(string(out))
	slices.Sort(f)
	return f
}

// running is the ids of this prefix's running containers.
func running(t *testing.T, prefix string) []string {
	t.Helper()
	return dockerLines(t, "ps", "-q", "--no-trunc", "--filter", "label=moor.product="+prefix)
}

func TestARunnerRestartLeavesTheTaskRunningAndReadoptsIt(t *testing.T) {
	h, prefix := realHost(t)
	g := rigWithin(t, 3*time.Minute)
	s, stop := g.start(h, h.connect)
	lines := watch(t, g.ctx, s)
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	await(t, g.ctx, lines, "the session live in the desk", sessionIs("t1", "live"))
	before := running(t, prefix)
	if len(before) != len(partNames) {
		t.Fatalf("%d containers running, want %d", len(before), len(partNames))
	}
	stop()
	if after := running(t, prefix); !slices.Equal(before, after) {
		t.Fatalf("the runner's exit changed the running containers: %v, then %v", before, after)
	}

	s, stop = g.start(h, h.connect)
	defer stop()
	var listed wire.TaskListReply
	if err := call(g.ctx, s, wire.KindTaskList, wire.TaskList{}, &listed); err != nil {
		t.Fatal(err)
	}
	if len(listed.Tasks) != 1 || listed.Tasks[0].Fence != (wire.Fence{Task: "t1", Run: 1, Gen: 1}) || len(listed.Tasks[0].Containers) != len(partNames) {
		t.Fatalf("the new runner lists %+v", listed)
	}
	lines = watch(t, g.ctx, s)
	await(t, g.ctx, lines, "the adopted session live", sessionIs("t1", "live"))
	if again := running(t, prefix); !slices.Equal(before, again) {
		t.Fatalf("re-adoption replaced containers: %v, then %v", before, again)
	}
	if err := call(g.ctx, s, wire.KindTaskStop, wire.TaskStop{Fence: wire.Fence{Task: "t1", Run: 1, Gen: 1}}, nil); err != nil {
		t.Fatal(err)
	}
	if left := running(t, prefix); len(left) != 0 {
		t.Fatalf("the stop left %v running", left)
	}
	nothingLeft(t, prefix)
}

func TestARevokedRunnerRemovesItsRealContainersNetworksAndVolumes(t *testing.T) {
	h, prefix := realHost(t)
	g := rigWithin(t, 3*time.Minute)
	r := newRunner(g.portal.Agent(), h, sessions{connect: h.connect, attach: h.attach, clear: h.clear, argv: []string{"harness"}})
	done := make(chan error, 1)
	go func() { done <- r.Run(g.ctx) }()
	s, err := g.portal.Next(g.ctx)
	if err != nil {
		t.Fatal(err)
	}
	lines := watch(t, g.ctx, s)
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	await(t, g.ctx, lines, "the session live in the desk", sessionIs("t1", "live"))
	if n := len(running(t, prefix)); n != len(partNames) {
		t.Fatalf("%d containers running, want %d", n, len(partNames))
	}
	g.portal.Revoke()
	g.portal.Drop()
	select {
	case err := <-done:
		if !errors.Is(err, runner.ErrRevoked) {
			t.Fatalf("the revoked runner ended with %v", err)
		}
	case <-g.ctx.Done():
		t.Fatal("the revoked runner kept running")
	}
	if left := running(t, prefix); len(left) != 0 {
		t.Fatalf("the revoked runner left %v running", left)
	}
	nothingLeft(t, prefix)
}

// viewers is how many hc attach processes the desk of task runs.
func viewers(t *testing.T, prefix, task string) int {
	t.Helper()
	out, err := exec.Command("docker", "exec", prefix+"-"+task+"-desk", "ps", "-eo", "args").CombinedOutput()
	if err != nil {
		t.Fatalf("ps in the desk: %v: %s", err, out)
	}
	return strings.Count(string(out), "hc attach")
}

func TestAdoptionClearsTheViewersARunnerThatDiedLeftInTheDesk(t *testing.T) {
	h, prefix := realHost(t)
	g := rigWithin(t, 3*time.Minute)
	g.clear = h.clear
	s, stop := g.start(h, h.connect)
	lines := watch(t, g.ctx, s)
	if err := call(g.ctx, s, wire.KindTaskStart, startMsg("t1", 1, 1), nil); err != nil {
		t.Fatal(err)
	}
	await(t, g.ctx, lines, "the session live in the desk", sessionIs("t1", "live"))
	viewer, err := h.attach(context.Background(), "t1", wire.Size{Cols: 80, Rows: 24}, "hc", "attach", sessionName, "--history", "10")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = viewer.Close() }()
	if n := viewers(t, prefix, "t1"); n != 1 {
		t.Fatalf("%d viewers in the desk, want the one just attached", n)
	}
	stop()
	if n := viewers(t, prefix, "t1"); n != 1 {
		t.Fatalf("a runner that ends left %d viewers, want it to leave the one it cannot reach", n)
	}
	_, stop = g.start(h, h.connect)
	defer stop()
	deadline := time.Now().Add(time.Minute)
	for viewers(t, prefix, "t1") != 0 {
		if time.Now().After(deadline) {
			t.Fatal("the new runner left the dead runner's viewer in the desk")
		}
		time.Sleep(250 * time.Millisecond)
	}
	if again := running(t, prefix); len(again) != len(partNames) {
		t.Fatalf("clearing the viewer changed the containers: %v", again)
	}
}
