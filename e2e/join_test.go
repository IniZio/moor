package e2e

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// shell is a command a person runs in a terminal: it ends with the terminal, and its exit is waited for.
type shell struct {
	cmd  *exec.Cmd
	done chan error
	log  string
}

func runInShell(t *testing.T, dir, name, bin string, args ...string) *shell {
	t.Helper()
	s := &shell{done: make(chan error, 1), log: filepath.Join(dir, name+".log")}
	f, err := os.Create(s.log)
	must(t, err)
	s.cmd = exec.Command(bin, args...)
	s.cmd.Stdout, s.cmd.Stderr = f, f
	must(t, s.cmd.Start())
	go func() { s.done <- s.cmd.Wait(); _ = f.Close() }()
	t.Cleanup(func() {
		_ = s.cmd.Process.Signal(syscall.SIGKILL)
		select {
		case err := <-s.done:
			s.done <- err
		case <-time.After(10 * time.Second):
		}
		if t.Failed() {
			b, _ := os.ReadFile(s.log)
			t.Logf("%s log:\n%s", name, tail(string(b), 40))
		}
	})
	return s
}

// closeShell ends the terminal: the shell's command gets the hangup a closing terminal sends it.
func (s *shell) closeShell(t *testing.T) {
	t.Helper()
	_ = s.cmd.Process.Signal(syscall.SIGTERM)
	select {
	case err := <-s.done:
		must(t, err)
	case <-time.After(30 * time.Second):
		t.Fatal("the runner did not end when its shell closed")
	}
}

// addMachine is Add machine in the portal and the join line it shows, as the arguments of the runner, run in the
// foreground so that the test owns the process. It returns the machine's id and the arguments.
func addMachine(t *testing.T, pt *portal, dir, prefix string, args []string) (string, []string) {
	t.Helper()
	var added struct {
		JoinCommand string `json:"join_command"`
	}
	pt.call(t, "POST", "/api/machines", nil, &added)
	m := joinLine.FindStringSubmatch(added.JoinCommand)
	if m == nil {
		t.Fatalf("join command %q", added.JoinCommand)
	}
	return m[2], append([]string{"join", "-portal", m[1], "-id", m[2], "-token", m[3], "-key", filepath.Join(dir, "runner.key"), "-prefix", prefix, "-foreground"}, args...)
}

func startPersonTask(t *testing.T, db *store.Store) domain.TaskID {
	t.Helper()
	tid, err := db.NextTaskID(context.Background())
	must(t, err)
	_, err = task.New(db, clock{}, task.Config{}).StartByPerson(context.Background(), task.StartInput{ID: tid, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: "Fix the checkout", By: "Mia"})
	must(t, err)
	return tid
}

func TestOneJoinLineEnrolsAndRunsTheRunnerAndARevokeRemovesTheTaskAndEndsTheRunner(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	prefix := "moore2e-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn}
	runnerBin := build(t, dir, "cmd/runner")
	src := startSource(t, dir, gitKey)
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": src.settings(settingsDoc, "")}, nil)
	id, line := addMachine(t, pt, dir, prefix, src.runnerArgs())

	first := runInShell(t, dir, "join-1", runnerBin, line...)
	eventually(t, time.Minute, "the machine linked after the one command", func() bool { return machineState(t, pt, id) == "linked" })
	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()
	tid := startPersonTask(t, db)
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, tid, domain.Running))
	before := running(t, prefix)

	first.closeShell(t)
	second := runInShell(t, dir, "join-2", runnerBin, line...)
	eventually(t, time.Minute, "the machine linked again by the same line, whose token is used", func() bool { return machineState(t, pt, id) == "linked" })
	time.Sleep(3 * time.Second)
	if after := running(t, prefix); len(after) != len(before) {
		t.Fatalf("the second join changed the containers: %v, then %v", before, after)
	}

	pt.call(t, "POST", "/api/machines/"+id+"/revoke", nil, nil)
	select {
	case err := <-second.done:
		must(t, err)
	case <-time.After(time.Minute):
		t.Fatal("the revoked runner kept running")
	}
	if left := remaining(t, prefix); len(left) != 0 {
		t.Fatalf("the revoked runner left %v on the host", left)
	}
	if got, _ := db.Load(context.Background(), tid); got.State != domain.Failed || got.Failure != "machine revoked" {
		t.Errorf("the task on the revoked machine is %s with %q", got.State, got.Failure)
	}
}
