package e2e

import (
	"context"
	"encoding/base64"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

const (
	sourceRepo = "/srv/git/example-org/shop-web.git"
	// seedSource makes the bare repository a task checks out: main and old, and a pull request head that no branch
	// holds, as a fork's commit is, under the numbers 7 and 90. Then it serves it.
	seedSource = `set -e; r=` + sourceRepo + `; git init -q --bare -b main $r; git -C $r config http.receivepack true
d=$(mktemp -d); cd "$d"; git init -q -b main; git config user.email seed@example.org; git config user.name seed
echo shop > README.md; git add README.md; git commit -qm seed; git push -q $r main main:old
git checkout -q -b fork; echo search > search.txt; git add search.txt; git commit -qm "Add search"
git push -q $r fork:refs/pull/7/head fork:refs/pull/90/head; exec /gitserver`
	// mintedToken is the token the fake GitHub mints for its one installation, limited to named repositories. A token
	// the App mints for itself has another value. It is never printed.
	mintedToken = "fake-token-4242"
)

// repoSource is the git server a test's tasks check out from, published on the Docker bridge's gateway so a task's
// Box egress proxy reaches it from the moment the task starts. addr is its host and port there.
type repoSource struct{ name, addr string }

// startSource serves the task's repository, refusing any request that lacks the Basic credential of secret. Its name
// holds no task prefix, so a check that a task's objects are gone does not count it.
func startSource(t *testing.T, dir, secret string) repoSource {
	t.Helper()
	gw := strings.TrimSpace(mustRun(t, "docker", "network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}"))
	name := "moorsrc-" + randHex(t, 4)
	auth := "Basic " + base64.StdEncoding.EncodeToString([]byte("x-access-token:"+secret))
	mustRun(t, "docker", "run", "-d", "--name", name, "-p", gw+"::80", "-u", "0", "-e", "EXPECT_AUTH="+auth,
		"-v", gitServerBin(t, dir)+":/gitserver:ro", "--entrypoint", "sh", image("desk"), "-c", seedSource)
	t.Cleanup(func() { _, _ = runCmd("docker", "rm", "-f", name) })
	src := repoSource{name: name, addr: strings.TrimSpace(mustRun(t, "docker", "port", name, "80/tcp"))}
	eventually(t, 30*time.Second, "the repository seeded", func() bool { return src.ref("refs/pull/7/head") != "" })
	return src
}

func gitServerBin(t *testing.T, dir string) string {
	t.Helper()
	bin := dir + "/gitserver"
	if _, err := os.Stat(bin); err == nil {
		return bin
	}
	cmd := exec.Command("go", "build", "-o", bin, "./testdata/gitserver")
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build the git server: %v: %s", err, out)
	}
	return bin
}

// runnerArgs are the image flags and the clone URL that points the runner's checkouts at the source.
func (s repoSource) runnerArgs() []string {
	return append(append([]string(nil), images...), "-clone-url", "http://"+s.addr+"/{path}.git")
}

// settings adds the source's rule to doc. With cred it names that credential, otherwise one of its own holding gitKey.
func (s repoSource) settings(doc, cred string) string {
	if cred == "" {
		cred = "checkout"
		entry := "  checkout:\n    value: " + gitKey + "\n"
		if strings.Contains(doc, "credentials:\n") {
			doc = strings.Replace(doc, "credentials:\n", "credentials:\n"+entry, 1)
		} else {
			doc = strings.Replace(doc, "rules:\n", "credentials:\n"+entry+"rules:\n", 1)
		}
	}
	rule := "  checkout:\n    hosts: [\"" + s.addr + "\"]\n    credential: " + cred +
		"\n    inject: {basic_user: x-access-token}\n    allow: [\"GET|POST /{repo}.git/**\"]\n"
	return strings.Replace(doc, "rules:\n", "rules:\n"+rule, 1)
}

func (s repoSource) ref(ref string) string {
	out, _ := runCmd("docker", "exec", s.name, "git", "--git-dir", sourceRepo, "rev-parse", "--verify", "-q", ref)
	return strings.TrimSpace(out)
}

func TestATaskStartsOnItsPullRequestHeadThroughAMintedTokenTheBoxNeverHolds(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	fakeURL, keyFile := fakeGitHub(t, dir)
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moorco-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	src := startSource(t, dir, mintedToken)
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": src.settings(appSettings(t, fakeURL, keyFile), "github")}, nil)
	keyPath := dir + "/runner.key"
	mid := joined(t, pt, runnerBin, keyPath)
	start(t, "runner", nil, runnerBin, append([]string{"-portal", "ws://" + pt.addr + "/runner/link", "-id", mid, "-key", keyPath, "-prefix", prefix}, src.runnerArgs()...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, mid) == "linked" })
	head := src.ref("refs/pull/7/head")
	id := prTask(t, dsn, head)
	box := prefix + "-" + string(id) + "-box"

	t.Run("the Box holds the repository at the pull request's head", func(t *testing.T) {
		if got := inBox(t, box, "cd /workspace && git rev-parse HEAD && cat search.txt"); got != head+"\nsearch" {
			t.Fatalf("the work tree is at %q, want %s with the pull request's file", got, head)
		}
	})
	t.Run("the token was minted for the task's repository alone", func(t *testing.T) { mintedForOneRepo(t, fakeURL) })
	t.Run("a feature branch lands and the default branch is refused", func(t *testing.T) { pushesFromTheWorkTree(t, box, src) })
	t.Run("the token is nowhere in the Box", func(t *testing.T) { tokenAbsent(t, box) })
	left := leftBehind(t, prefix)
	t.Logf("the test removed %d containers, %d networks and %d volumes", len(left["container"]), len(left["network"]), len(left["volume"]))
	if again := remaining(t, prefix); len(again) != 0 {
		t.Errorf("still on the host after the removal: %v", again)
	}
}

// prTask starts a task for pull request 7 of the repository, the way a webhook's event would, and waits for it to run.
func prTask(t *testing.T, dsn, head string) domain.TaskID {
	t.Helper()
	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	t.Cleanup(db.Close)
	id, err := db.NextTaskID(context.Background())
	must(t, err)
	_, err = task.New(db, clock{}, task.Config{}).StartByEvent(context.Background(), task.EventInput{
		ID: id, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"},
		Spec:  domain.Spec{Name: "PR evidence", File: "tasks/pr-evidence.yaml"},
		Event: domain.Event{Source: "github", Group: "example-org/shop-web#7", Rev: domain.Revision(head)},
		Ref:   "refs/pull/7/head", Commit: domain.Commit(head), Prompt: "Show evidence",
	})
	must(t, err)
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, id, domain.Running))
	return id
}

func mintedForOneRepo(t *testing.T, fakeURL string) {
	var asked []struct {
		Repositories []string `json:"repositories"`
	}
	getJSON(t, fakeURL+"/_fake/tokens", &asked)
	scoped := 0
	for _, a := range asked {
		if len(a.Repositories) == 0 {
			continue
		}
		scoped++
		if len(a.Repositories) != 1 || a.Repositories[0] != "shop-web" {
			t.Errorf("a task token was minted for %v", a.Repositories)
		}
	}
	if scoped == 0 {
		t.Error("no token limited to the task's repository was minted, though the git server takes only that one")
	}
}

func pushesFromTheWorkTree(t *testing.T, box string, src repoSource) {
	line := `cd /workspace && git config user.email agent@example.org && git config user.name agent && git checkout -q -b add-search && ` +
		`echo more >> search.txt && git commit -qam "More search" && git push -q origin add-search 2>&1; echo rc=$?`
	if out := inBox(t, box, line); !strings.HasSuffix(out, "rc=0") {
		t.Fatalf("the feature push: %s", out)
	}
	if local := inBox(t, box, "cd /workspace && git rev-parse HEAD"); src.ref("refs/heads/add-search") != local {
		t.Fatalf("the server lacks the pushed branch at %s", local)
	}
	before := src.ref("refs/heads/main")
	out := inBox(t, box, "cd /workspace && git push origin HEAD:main 2>&1; echo rc=$?")
	if strings.HasSuffix(out, "rc=0") || !strings.Contains(out, "pushing to the default branch is not allowed from a task") {
		t.Errorf("the push to main:\n%s", out)
	}
	if src.ref("refs/heads/main") != before {
		t.Error("the refused push moved main")
	}
}

// tokenScan counts the files, and the environments and command lines of processes, in the Box that hold the token
// read from stdin. The token stays in an unexported shell variable, so the scan itself holds it in no process record.
const tokenScan = `read -r tok
printf '%s\n' "$tok" | grep -rlsF -f - / --exclude-dir=proc --exclude-dir=sys --exclude-dir=dev | wc -l
n=0
for f in /proc/[0-9]*/environ /proc/[0-9]*/cmdline; do
	case "$(tr '\0' ' ' < "$f" 2>/dev/null)" in *"$tok"*) n=$((n+1)) ;; esac
done
echo "$n"`

// tokenAbsent counts where the minted token shows in the Box, then plants it in one file to show the scan finds it
// there. It never prints the token.
func tokenAbsent(t *testing.T, box string) {
	if got := inBoxWithToken(t, box, tokenScan); len(got) != 2 || got[0] != "0" || got[1] != "0" {
		t.Fatalf("the token shows in %v files and process records of the Box", got)
	}
	inBoxWithToken(t, box, `read -r tok; printf '%s' "$tok" > /var/tmp/planted`)
	if got := inBoxWithToken(t, box, tokenScan); len(got) != 2 || got[0] != "1" {
		t.Fatalf("the scan found %v with the token planted in one file", got)
	}
}

// inBoxWithToken runs script in the Box as root with the token on its stdin, and gives the words it printed.
func inBoxWithToken(t *testing.T, box, script string) []string {
	t.Helper()
	cmd := exec.Command("docker", "exec", "-i", "-u", "0", box, "sh", "-c", script)
	cmd.Stdin = strings.NewReader(mintedToken + "\n")
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("in the Box: %v", err)
	}
	return strings.Fields(string(out))
}
