//go:build realrun

package e2e

import (
	"context"
	"encoding/base64"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// The real run: one person-started task worked by a real harness against the model, with the credential in
// MOOR_REALRUN_TOKEN_FILE. It never prints the credential; it only counts where it appears. MOOR_REALRUN_OUT, when
// set, is a folder for the run's status and screen, and MOOR_E2E_HOLD a file whose creation ends the hold at the end.

const (
	fixtureRepo = "/srv/git/example-org/shop-web.git"
	seedFixture = `set -e; r=` + fixtureRepo + `; git init -q --bare -b main $r; git -C $r config http.receivepack true
d=$(mktemp -d); cd "$d"; git init -q -b main; git config user.email seed@example.org; git config user.name seed
printf '# sum\n\nA tiny library that adds two numbers. Run the tests with npm test.\n' > README.md
printf '{"name": "sum", "version": "1.0.0", "scripts": {"test": "node --test"}}\n' > package.json
printf 'module.exports = (a, b) => a - b;\n' > sum.js
printf "const test = require('node:test');\nconst assert = require('node:assert');\nconst sum = require('./sum');\n\ntest('adds two numbers', () => assert.strictEqual(sum(2, 3), 5));\n" > sum.test.js
git add -A; git commit -qm "Add sum"; git push -q $r main; exec /gitserver`
	realPrompt = "The test in this repository fails. Fix the code so the test passes, run the tests, commit the fix on a " +
		"new branch named fix-sum, push that branch to origin, and publish a result page."
	realReview = "Check your page before it is accepted: does it show the failing test output before the fix, the passing " +
		"output after it, and the pushed branch with its commit? Fix what is missing and publish again."
)

func modelRule(token string) (rule, cred string) {
	return "  anthropic:\n    hosts: [api.anthropic.com]\n    credential: anthropic\n" +
			"    inject: {header: Authorization, form: \"Bearer {secret}\"}\n    allow: \"*\"\n" +
			"    box_env: {CLAUDE_CODE_OAUTH_TOKEN: placeholder}\n",
		"  anthropic:\n    value: \"" + token + "\"\n"
}

func TestARealHarnessWorksATask(t *testing.T) {
	tokenFile := os.Getenv("MOOR_REALRUN_TOKEN_FILE")
	if os.Getenv(dockerEnv) != "1" || tokenFile == "" {
		t.Skip("the real run needs MOOR_TEST_DOCKER=1 and MOOR_REALRUN_TOKEN_FILE")
	}
	raw, err := os.ReadFile(tokenFile)
	must(t, err)
	token := strings.TrimSpace(string(raw))
	out := os.Getenv("MOOR_REALRUN_OUT")
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	fakeURL, keyFile := fakeGitHub(t, dir)
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moor-test-rr" + randHex(t, 3)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Logf("removed: %v", left)
		}
	})
	src := startFixture(t, dir)
	pt.start(t)
	rule, cred := modelRule(token)
	doc := appSettings(t, fakeURL, keyFile)
	doc = strings.Replace(strings.Replace(doc, "rules:\n", "rules:\n"+rule, 1), "credentials:\n", "credentials:\n"+cred, 1)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": src.settings(doc, "")}, nil)
	keyPath := dir + "/runner.key"
	mid := joined(t, pt, runnerBin, keyPath)
	rn := start(t, "runner", nil, runnerBin, append([]string{"-portal", "ws://" + pt.addr + "/runner/link", "-id", mid, "-key", keyPath, "-prefix", prefix, "-harness", "claude"}, src.runnerArgs()...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, mid) == "linked" })
	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()
	id, err := db.NextTaskID(context.Background())
	must(t, err)
	t0 := time.Now()
	_, err = task.New(db, clock{}, task.Config{}).StartByPerson(context.Background(), task.StartInput{ID: id, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: realPrompt, ReviewPrompt: realReview, By: "Mia"})
	must(t, err)
	status(t, out, fmt.Sprintf("portal=http://%s task=%s prefix=%s user-header=%s\n", pt.addr, id, prefix, userHeader))
	eventually(t, 5*time.Minute, "the task running", taskIs(t, db, id, domain.Running))
	t.Logf("running after %s", time.Since(t0).Round(time.Second))
	ended := waitEnd(t, db, id, 40*time.Minute)
	t.Logf("%s after %s", ended.State, time.Since(t0).Round(time.Second))
	box := prefix + "-" + string(id) + "-box"
	desk := prefix + "-" + string(id) + "-desk"
	report(t, pt, db, id, box, desk, src, out)
	absent(t, token, box, desk, []string{pt.p.log, rn.log})
	holdForABrowser(t, pt.addr, string(id))
}

func status(t *testing.T, out, line string) {
	t.Log(strings.TrimSpace(line))
	if out != "" {
		must(t, os.WriteFile(filepath.Join(out, "status"), []byte(line), 0o600))
	}
}

func waitEnd(t *testing.T, db *store.Store, id domain.TaskID, limit time.Duration) domain.Task {
	var got domain.Task
	eventually(t, limit, "the task ended", func() bool {
		var err error
		got, err = db.Load(context.Background(), id)
		must(t, err)
		return got.State == domain.Done || got.State == domain.Failed || got.State == domain.Cancelled
	})
	if got.State != domain.Done {
		t.Errorf("the task is %s: %s", got.State, got.Failure)
	}
	return got
}

func report(t *testing.T, pt *portal, db *store.Store, id domain.TaskID, box, desk string, src repoSource, out string) {
	var snap snapshot
	pt.call(t, "GET", "/api/tasks/"+string(id), nil, &snap)
	for _, v := range snap.Versions {
		t.Logf("version %d: %s", v.Version, v.Line)
	}
	if len(snap.Versions) < 2 {
		t.Errorf("%d versions, want a first one and one after the review", len(snap.Versions))
	}
	t.Logf("notes: %q", eventTexts(snap, "note"))
	t.Logf("tests in the Box: %s", inBox(t, box, "cd /workspace && npm test 2>&1 | tail -n 8; echo rc=$?"))
	t.Logf("work tree: %s", inBox(t, box, "cd /workspace && git status -sb | head -5 && git log --oneline -3"))
	if head := src.ref("refs/heads/fix-sum"); head == "" {
		t.Errorf("the server has no fix-sum branch")
	} else {
		t.Logf("fix-sum on the server at %s", head)
	}
	blocked, err := db.BlockedRequests(context.Background(), id)
	must(t, err)
	for _, b := range blocked {
		t.Logf("blocked on the %s proxy: %s %s:%d%s (%s) x%d", b.Proxy, b.Method, b.Host, b.Port, b.Path, b.Reason, b.Count)
	}
	shells, _ := runCmd("docker", "exec", desk, "sh", "-c", "grep -c START /desk/log/wrapper.log; grep START /desk/log/wrapper.log | cut -c1-160")
	t.Logf("shell commands sent to the Box:\n%s", shells)
	if out != "" {
		must(t, os.WriteFile(filepath.Join(out, "screen.txt"), []byte(screen(t, pt, id)), 0o600))
	}
}

// absent counts the places that hold the credential, giving the counter the credential on a pipe, never an argument.
func absent(t *testing.T, token, box, desk string, logs []string) {
	inside := map[string]string{
		"env":       "env | grep -cF -f /dev/fd/3",
		"processes": "cat /proc/[0-9]*/environ /proc/[0-9]*/cmdline 2>/dev/null | tr '\\0' '\\n' | grep -cF -f /dev/fd/3",
		"files":     "grep -rlaF -f /dev/fd/3 / --exclude-dir=proc --exclude-dir=sys --exclude-dir=dev 2>/dev/null | wc -l",
	}
	for _, c := range []string{box, desk} {
		for what, script := range inside {
			n := counted(t, token, nil, "docker", "exec", "-i", "-u", "0", c, "sh", "-c", "exec 3<&0 </dev/null; "+script)
			t.Logf("the credential in %s %s: %d", c, what, n)
			if n != 0 {
				t.Errorf("the credential is in %s %s", c, what)
			}
		}
	}
	for _, l := range logs {
		f, err := os.Open(l)
		must(t, err)
		n := counted(t, token, f, "grep", "-cF", "-f", "/dev/fd/3")
		_ = f.Close()
		t.Logf("the credential in %s: %d", filepath.Base(l), n)
		if n != 0 {
			t.Errorf("the credential is in the log %s", filepath.Base(l))
		}
	}
	dump := exec.Command("docker", "run", "--rm", "--network", "host", "postgres:17", "pg_dump", os.Getenv("MOOR_TEST_POSTGRES_DSN"))
	pipe, err := dump.StdoutPipe()
	must(t, err)
	must(t, dump.Start())
	n := counted(t, token, pipe, "grep", "-cF", "-f", "/dev/fd/3")
	_ = dump.Wait()
	t.Logf("the credential in the Postgres dump: %d", n)
	if n != 0 {
		t.Errorf("the credential is in the database in the clear")
	}
}

// counted runs name with the credential on its fd 3 and data, when given, on its stdin, or the credential there too.
func counted(t *testing.T, token string, data io.Reader, name string, args ...string) int {
	t.Helper()
	pr, pw, err := os.Pipe()
	must(t, err)
	cmd := exec.Command(name, args...)
	cmd.ExtraFiles = []*os.File{pr}
	if data != nil {
		cmd.Stdin = data
	} else {
		cmd.Stdin = strings.NewReader(token + "\n")
	}
	go func() { _, _ = pw.WriteString(token + "\n"); _ = pw.Close() }()
	b, _ := cmd.Output()
	_ = pr.Close()
	n, err := strconv.Atoi(strings.TrimSpace(string(b)))
	if err != nil {
		t.Fatalf("%s gave no count", name)
	}
	return n
}

// startFixture serves the fixture repository: a tiny Node library whose one test fails.
func startFixture(t *testing.T, dir string) repoSource {
	t.Helper()
	gw := strings.TrimSpace(mustRun(t, "docker", "network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}"))
	name := "moor-test-src" + randHex(t, 3)
	auth := "Basic " + base64.StdEncoding.EncodeToString([]byte("x-access-token:"+gitKey))
	mustRun(t, "docker", "run", "-d", "--name", name, "-p", gw+"::80", "-u", "0", "-e", "EXPECT_AUTH="+auth,
		"-v", gitServerBin(t, dir)+":/gitserver:ro", "--entrypoint", "sh", image("desk"), "-c", seedFixture)
	t.Cleanup(func() { _, _ = runCmd("docker", "rm", "-f", name) })
	src := repoSource{name: name, addr: strings.TrimSpace(mustRun(t, "docker", "port", name, "80/tcp"))}
	eventually(t, 30*time.Second, "the fixture seeded", func() bool { return src.ref("refs/heads/main") != "" })
	return src
}
