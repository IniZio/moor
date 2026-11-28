package e2e

import (
	"context"
	"encoding/base64"
	"io"
	"net/http"
	"os"
	"os/exec"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// gitKey is the synthetic value of the git server's credential. It is never printed.
const gitKey = "synthetic-git-7c3e91d0"

const (
	pushRepo = "/srv/git/example-org/shop-web.git"
	// seedRepo makes the bare repository with main and a branch named old, then serves it.
	seedRepo = `set -e; git init -q --bare -b main ` + pushRepo + `; git -C ` + pushRepo + ` config http.receivepack true
d=$(mktemp -d); git -C "$d" init -q -b main; git -C "$d" -c user.email=seed@example.org -c user.name=seed commit -q --allow-empty -m seed
git -C "$d" push -q ` + pushRepo + ` main main:old; exec /gitserver`
	// clone makes a work tree of the repository in the Box, on a branch of its own with one new commit.
	clone = `cd /tmp && rm -rf w && git clone -q http://git.shop.test/example-org/shop-web.git w && cd w && ` +
		`git config user.email agent@example.org && git config user.name agent && git checkout -q -b fix-checkout && ` +
		`echo fixed > checkout.txt && git add checkout.txt && git commit -qm "Fix the checkout"`
)

// pushDoc is the settings: the git server's rule and credential, and the GitHub App on the fake GitHub, which
// the portal reads the default branch from.
func pushDoc(t *testing.T, fakeURL, keyFile string) string {
	app := strings.SplitN(appSettings(t, fakeURL, keyFile), "credentials:\n", 2)[1]
	return "repositories:\n  - example-org/shop-web\ncredentials:\n  git:\n    value: " + gitKey + "\n" + app + `rules:
  git:
    hosts: ["git.shop.test:80"]
    credential: git
    inject: {basic_user: x-access-token}
    allow: ["GET|POST /{repo}.git/**"]
`
}

// gitServer starts the git server on the task's egress network as git.shop.test.
func gitServer(t *testing.T, dir, prefix string, id domain.TaskID) string {
	t.Helper()
	bin := dir + "/gitserver"
	cmd := exec.Command("go", "build", "-o", bin, "./testdata/gitserver")
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build the git server: %v: %s", err, out)
	}
	name := prefix + "-gitserver"
	auth := "Basic " + base64.StdEncoding.EncodeToString([]byte("x-access-token:"+gitKey))
	mustRun(t, "docker", "run", "-d", "--name", name, "--network", "none", "-u", "0", "-e", "EXPECT_AUTH="+auth,
		"-v", bin+":/gitserver:ro", "--entrypoint", "sh", image("desk"), "-c", seedRepo)
	t.Cleanup(func() { _, _ = runCmd("docker", "rm", "-f", name) })
	mustRun(t, "docker", "network", "disconnect", "none", name)
	mustRun(t, "docker", "network", "connect", "--alias", "git.shop.test", prefix+"-"+string(id)+"-egress", name)
	return name
}

func TestATaskPushesItsBranchButNeverTheDefaultBranchADeleteOrATag(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	fakeURL, keyFile := fakeGitHub(t, dir)
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moorpush-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": pushDoc(t, fakeURL, keyFile)}, nil)
	keyPath := dir + "/runner.key"
	mid := joined(t, pt, runnerBin, keyPath)
	start(t, "runner", nil, runnerBin, append([]string{"-portal", "ws://" + pt.addr + "/runner/link", "-id", mid, "-key", keyPath, "-prefix", prefix}, images...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, mid) == "linked" })
	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()
	id, err := db.NextTaskID(context.Background())
	must(t, err)
	_, err = task.New(db, clock{}, task.Config{}).StartByPerson(context.Background(), task.StartInput{ID: id, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: "Fix the checkout", By: "Mia"})
	must(t, err)
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, id, domain.Running))
	srv := gitServer(t, dir, prefix, id)
	box := prefix + "-" + string(id) + "-box"
	if out := inBox(t, box, clone+"; echo rc=$?"); !strings.HasSuffix(out, "rc=0") {
		t.Fatalf("the clone in the Box: %s", out)
	}
	t.Run("a feature branch is pushed and lands on the server", func(t *testing.T) { featurePushed(t, box, srv) })
	t.Run("the default branch, a delete and a tag are refused with the reason in git's output", func(t *testing.T) { refusedPushes(t, box, srv) })
	t.Run("each refusal shows on the task without Allow", func(t *testing.T) { refusalsOnTheTask(t, pt, id) })
	t.Run("a 40 MB push streams through", func(t *testing.T) { largePush(t, box, srv) })
	left := leftBehind(t, prefix)
	t.Logf("the test removed %d containers, %d networks and %d volumes", len(left["container"]), len(left["network"]), len(left["volume"]))
	if again := remaining(t, prefix); len(again) != 0 {
		t.Errorf("still on the host after the removal: %v", again)
	}
}

func serverRef(t *testing.T, srv, ref string) string {
	out, _ := runCmd("docker", "exec", srv, "git", "--git-dir", pushRepo, "rev-parse", "--verify", "-q", ref)
	return strings.TrimSpace(out)
}

func featurePushed(t *testing.T, box, srv string) {
	if out := inBox(t, box, "cd /tmp/w && git push -q origin fix-checkout 2>&1; echo rc=$?"); !strings.HasSuffix(out, "rc=0") {
		t.Fatalf("the feature push: %s", out)
	}
	if local, remote := inBox(t, box, "cd /tmp/w && git rev-parse HEAD"), serverRef(t, srv, "refs/heads/fix-checkout"); local == "" || local != remote {
		t.Fatalf("the server holds %q for the branch, the Box %q", remote, local)
	}
}

func refusedPushes(t *testing.T, box, srv string) {
	mainBefore := serverRef(t, srv, "refs/heads/main")
	for line, reason := range map[string]string{
		"git push origin HEAD:main":                "pushing to the default branch is not allowed from a task",
		"git push origin :old":                     "deleting a branch is not allowed from a task",
		"git tag v1.0.0 && git push origin v1.0.0": "pushing tags is not allowed from a task",
	} {
		out := inBox(t, box, "cd /tmp/w && "+line+" 2>&1; echo rc=$?")
		if strings.HasSuffix(out, "rc=0") || !strings.Contains(out, "("+reason+")") {
			t.Errorf("%s:\n%s", line, out)
		}
		t.Logf("%s:\n%s", line, out)
	}
	if serverRef(t, srv, "refs/heads/main") != mainBefore || serverRef(t, srv, "refs/heads/old") == "" || serverRef(t, srv, "refs/tags/v1.0.0") != "" {
		t.Errorf("a refused push changed the server")
	}
}

func refusalsOnTheTask(t *testing.T, pt *portal, id domain.TaskID) {
	want := []string{"pushing to the default branch is not allowed from a task", "deleting a branch is not allowed from a task", "pushing tags is not allowed from a task"}
	var snap struct {
		Blocked []blockedRow `json:"blocked_requests"`
	}
	eventually(t, 30*time.Second, "three refused pushes on the task", func() bool {
		pt.call(t, "GET", "/api/tasks/"+string(id), nil, &snap)
		seen := 0
		for _, b := range snap.Blocked {
			if b.Reason == "push_not_allowed" && b.Request == "git.shop.test:80 POST /example-org/shop-web.git/git-receive-pack" && slices.Contains(want, b.Detail) {
				seen++
			}
		}
		return seen == len(want)
	})
	page := taskPage(t, pt, id)
	for _, w := range want {
		if !strings.Contains(page, w) {
			t.Errorf("the task's page lacks %q", w)
		}
	}
	if strings.Contains(page, ">Allow<") {
		t.Errorf("the task's page offers Allow on a refused push")
	}
}

// taskPage is the task's page as the signed-in person Mia sees it.
func taskPage(t *testing.T, pt *portal, id domain.TaskID) string {
	t.Helper()
	req, err := http.NewRequest(http.MethodGet, "http://"+pt.addr+"/tasks/"+string(id), nil)
	must(t, err)
	req.Header.Set(userHeader, "Mia")
	resp, err := http.DefaultClient.Do(req)
	must(t, err)
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(resp.Body)
	return string(b)
}

func largePush(t *testing.T, box, srv string) {
	line := "cd /tmp/w && head -c 40000000 /dev/urandom > big.bin && git add big.bin && git commit -qm big && git push -q origin HEAD:big 2>&1; echo rc=$?"
	if out := inBox(t, box, line); !strings.HasSuffix(out, "rc=0") {
		px, _ := runCmd("docker", "logs", "--tail", "20", strings.TrimSuffix(box, "-box")+"-boxproxy")
		gs, _ := runCmd("docker", "logs", "--tail", "20", srv)
		rl, _ := runCmd("docker", "logs", "--tail", "20", strings.TrimSuffix(box, "-box")+"-relay")
		t.Fatalf("the large push: %s\nproxy:\n%s\nserver:\n%s\nrelay:\n%s", out, px, gs, rl)
	}
	if local, remote := inBox(t, box, "cd /tmp/w && git rev-parse HEAD"), serverRef(t, srv, "refs/heads/big"); local != remote {
		t.Fatalf("the server holds %q for the large branch, the Box %q", remote, local)
	}
}
