package e2e

import (
	"context"
	"os"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

const settingsDoc = `repositories:
  - example-org/shop-web
allow:
  - registry.npmjs.org:443 GET
`

var joinLine = regexp.MustCompile(`^runner join -portal (\S+) -id (\S+) -token (\S+) -key runner\.key$`)

// images are the runner's image flags for the images make images built, under the tag make passes in
// MOOR_IMAGE_TAG.
var images = []string{"-box-image", image("box"), "-desk-image", image("desk"), "-relay-image", image("relay"), "-proxy-image", image("proxy")}

func image(name string) string {
	tag := os.Getenv("MOOR_IMAGE_TAG")
	if tag == "" {
		tag = "dev"
	}
	return "moor-fresh-" + name + ":" + tag
}

type clock struct{}

func (clock) Now() time.Time { return time.Now().UTC() }

// joined enrols a machine through the portal and runs the real join command with a fresh key.
func joined(t *testing.T, pt *portal, runnerBin, keyFile string) string {
	t.Helper()
	var added struct {
		JoinCommand string `json:"join_command"`
	}
	pt.call(t, "POST", "/api/machines", nil, &added)
	m := joinLine.FindStringSubmatch(added.JoinCommand)
	if m == nil {
		t.Fatalf("join command %q", added.JoinCommand)
	}
	out, err := runCmd(runnerBin, "join", "-enrol-only", "-portal", m[1], "-id", m[2], "-token", m[3], "-key", keyFile, "-name", "e2e-"+randHex(t, 4))
	if err != nil {
		t.Fatalf("runner join: %v: %s", err, out)
	}
	return m[2]
}

func machineState(t *testing.T, pt *portal, id string) string {
	var rows struct {
		Machines []struct{ ID, State string } `json:"machines"`
	}
	pt.call(t, "GET", "/api/machines", nil, &rows)
	for _, r := range rows.Machines {
		if r.ID == id {
			return r.State
		}
	}
	return ""
}

func TestATaskRunsOnARealRunnerOutlivesAPortalRestartAndIsCancelled(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: freeAddr(t), dsn: dsn}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moore2e-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": settingsDoc}, nil)
	keyFile := dir + "/runner.key"
	id := joined(t, pt, runnerBin, keyFile)
	start(t, "runner", nil, runnerBin, append([]string{
		"-portal", "ws://" + pt.addr + "/runner/link", "-id", id,
		"-key", keyFile, "-prefix", prefix,
	}, images...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, id) == "linked" })

	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()
	svc := task.New(db, clock{}, task.Config{})
	tid, err := db.NextTaskID(context.Background())
	must(t, err)
	_, err = svc.StartByPerson(context.Background(), task.StartInput{ID: tid, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: "Fix the checkout", By: "Mia"})
	must(t, err)
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, tid, domain.Running))
	before := running(t, prefix)
	if len(before) != 5 {
		t.Fatalf("%d containers running, want desk, Box, relay and two proxies", len(before))
	}
	sessionHasThePromptAndTheBox(t, prefix, tid)

	pt.p.stop(t)
	pt.start(t)
	eventually(t, time.Minute, "the machine linked to the new portal", func() bool { return machineState(t, pt, id) == "linked" })
	time.Sleep(3 * time.Second)
	if got, _ := db.Load(context.Background(), tid); got.State != domain.Running || got.Run != 1 {
		t.Fatalf("after the portal restart the task is %s at run %d", got.State, got.Run)
	}
	if after := running(t, prefix); !slices.Equal(before, after) {
		t.Fatalf("the portal restart changed the containers: %v, then %v", before, after)
	}

	_, err = svc.Cancel(context.Background(), tid)
	must(t, err)
	eventually(t, time.Minute, "the cancelled task's containers, networks and volumes removed", func() bool { return len(remaining(t, prefix)) == 0 })
}

func taskIs(t *testing.T, db *store.Store, id domain.TaskID, want domain.State) func() bool {
	return func() bool {
		got, err := db.Load(context.Background(), id)
		must(t, err)
		if got.State == domain.Failed {
			t.Fatalf("the task failed: %s", got.Failure)
		}
		return got.State == want
	}
}

// sessionHasThePromptAndTheBox reads the environment of the harness the runner started in the desk.
// The desk starts no session itself, so this is the runner's session.
func sessionHasThePromptAndTheBox(t *testing.T, prefix string, id domain.TaskID) {
	t.Helper()
	desk := prefix + "-" + string(id) + "-desk"
	script := `for p in /proc/[0-9]*; do grep -qs standin "$p/cmdline" && tr '\0' '\n' < "$p/environ"; done`
	var env string
	eventually(t, time.Minute, "the harness running in the desk", func() bool {
		out, err := runCmd("docker", "exec", desk, "bash.real", "-c", script)
		env = out
		return err == nil && strings.Contains(out, "TASK_PROMPT=")
	})
	for _, want := range []string{"TASK_PROMPT=Fix the checkout", "BOX_SSH=rootless@" + prefix + "-" + string(id) + "-box"} {
		if !strings.Contains(env, want+"\n") {
			t.Errorf("the harness environment lacks %q:\n%s", want, env)
		}
	}
}
