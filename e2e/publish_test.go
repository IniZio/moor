package e2e

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
)

// script is what the stand-in plays: publish, wait for the review prompt, publish again, wait for a teammate's line,
// publish a third time.
const script = `publish /desk/page1.html Four of five flows passed.
wait Check your page
publish /desk/page2.html Five of five flows passed.
wait one more thing
publish /desk/page3.html Five of five flows passed, and one more.
`

type snapshot struct {
	Task struct {
		State string `json:"state"`
	} `json:"task"`
	Versions []struct {
		Version  int    `json:"version"`
		EntryURL string `json:"entry_url"`
		Line     string `json:"line"`
	} `json:"versions"`
	Events []struct {
		Kind string          `json:"kind"`
		Data json.RawMessage `json:"data"`
	} `json:"events"`
}

func eventTexts(s snapshot, kind string) []string {
	var out []string
	for _, e := range s.Events {
		var d struct{ Text, Target string }
		if e.Kind == kind && json.Unmarshal(e.Data, &d) == nil {
			out = append(out, d.Text+d.Target)
		}
	}
	return out
}

func TestAnAgentPublishesAndGetsTheReviewTurnAndTheCommentIsPostedOnceThenEdited(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	fakeURL, keyFile := fakeGitHub(t, dir)
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn, env: []string{"PORTAL_HOME_DIR=" + homeWith(t, dir)}}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moore9a-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": appSettings(t, fakeURL, keyFile)}, nil)
	keyPath := dir + "/runner.key"
	mid := joined(t, pt, runnerBin, keyPath)
	start(t, "runner", nil, runnerBin, append([]string{"-portal", "ws://" + pt.addr + "/runner/link", "-id", mid, "-key", keyPath, "-prefix", prefix}, images...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, mid) == "linked" })

	id := openedPullRequest(t, pt)
	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, domain.TaskID(id), domain.Running))
	desk := prefix + "-" + id + "-desk"
	play(t, desk)

	var snap snapshot
	read := func() { pt.call(t, "GET", "/api/tasks/"+id, nil, &snap) }
	eventually(t, 2*time.Minute, "the task done with two versions", func() bool {
		read()
		return snap.Task.State == "done" && len(snap.Versions) == 2
	})
	t.Run("the review prompt arrived once", func(t *testing.T) { reviewOnce(t, pt, id, desk, snap) })
	t.Run("the comment was created once with the accepted version", func(t *testing.T) { commentCreated(t, fakeURL, id, snap) })
	t.Run("the Run page shows set up notes, two versions and the answer", func(t *testing.T) { runShows(t, snap) })
	t.Run("a served page is the published one, sandboxed", func(t *testing.T) { pageServed(t, pt, snap) })
	t.Run("a teammate's line reopens it and the comment is edited, not made again", func(t *testing.T) { reopened(t, pt, fakeURL, id) })
	t.Run("a person cannot publish", func(t *testing.T) { personRefused(t, pt, id) })
	t.Run("the desk holds no trace of the task token", func(t *testing.T) { noTaskToken(t, desk, prefix+"-"+id+"-deskproxy") })
	holdForABrowser(t, pt.addr, id)
	left := leftBehind(t, prefix)
	t.Logf("the done task keeps its desk and Box for the keep window, which nothing ends yet, so the test removed %d containers, %d networks and %d volumes", len(left["container"]), len(left["network"]), len(left["volume"]))
	if again := remaining(t, prefix); len(again) != 0 {
		t.Errorf("still on the host after the removal: %v", again)
	}
}

// play gives the stand-in in the desk its pages and its script, and starts it.
func play(t *testing.T, desk string) {
	for n, name := range []string{"page1", "page2", "page3"} {
		deskWrite(t, desk, "/desk/"+name+".html", "<h1>Evidence v"+string(rune('1'+n))+"</h1><p>An agent drove the app. 5 < 6 & ok.</p>")
	}
	deskWrite(t, desk, "/desk/script", script)
	deskWrite(t, desk, "/desk/go", "")
}

// appSettings is the settings a person saves: the repository, the rules of the task and the GitHub App, whose API
// is the fake GitHub.
func appSettings(t *testing.T, api, keyFile string) string {
	t.Helper()
	key, err := os.ReadFile(keyFile)
	must(t, err)
	indented := "        " + strings.ReplaceAll(strings.TrimSpace(string(key)), "\n", "\n        ")
	return settingsDoc + "credentials:\n  github:\n    github_app:\n      app_id: 17\n      private_key: |\n" + indented +
		"\n      webhook_secret: " + hookSecret + "\n      api_url: " + api + "/\n"
}

func reviewOnce(t *testing.T, pt *portal, id, desk string, snap snapshot) {
	if n := len(eventTexts(snap, "note")) - len(setUpNotes(snap)); n != 1 {
		t.Errorf("%d notes beyond the set up ones, want the one review note: %v", n, eventTexts(snap, "note"))
	}
	played := mustRun(t, "docker", "exec", desk, "cat", "/desk/play.out")
	if strings.Count(played, "heard Check your page") != 1 || strings.Count(played, "publish rc=0") < 2 {
		t.Errorf("the stand-in's record:\n%s", played)
	}
	typed := strings.Count(screenText(t, pt, id), "standin heard: Check your page")
	if typed != 1 {
		t.Errorf("the review prompt shows %d times on the session's screen", typed)
	}
}

func setUpNotes(s snapshot) []string {
	var out []string
	for _, n := range eventTexts(s, "note") {
		if strings.HasPrefix(n, "desk up") || n == "box up" || n == "prompt sent" {
			out = append(out, n)
		}
	}
	return out
}

func commentCreated(t *testing.T, fakeURL, id string, snap snapshot) {
	if c, e := fakeWrites(t, fakeURL); c != 1 || e != 0 {
		t.Fatalf("%d creates and %d edits, want 1 and 0", c, e)
	}
	comments := fakeComments(t, fakeURL)
	if len(comments) != 1 || !strings.Contains(comments[0].Body, "Five of five flows passed.") || !strings.Contains(comments[0].Body, "/results/"+id+"/2") || strings.Contains(comments[0].Body, "Four of five") {
		t.Fatalf("comments %+v", comments)
	}
}

func runShows(t *testing.T, snap snapshot) {
	notes := setUpNotes(snap)
	if len(notes) != 3 || !strings.HasPrefix(notes[0], "desk up: m-") || notes[1] != "box up" || notes[2] != "prompt sent" {
		t.Errorf("set up notes %v", notes)
	}
	if len(snap.Versions) != 2 || snap.Versions[0].Line != "Four of five flows passed." || snap.Versions[1].Line != "Five of five flows passed." {
		t.Errorf("versions %+v", snap.Versions)
	}
	if d := eventTexts(snap, "delivered"); len(d) != 1 {
		t.Errorf("delivered rows %v", d)
	}
}

// holdForABrowser keeps the stack up while a person looks at it, when MOOR_E2E_HOLD names a file, until that file exists.
func holdForABrowser(t *testing.T, addr, id string) {
	hold := os.Getenv("MOOR_E2E_HOLD")
	if hold == "" {
		return
	}
	t.Logf("HOLD portal=http://%s task=%s until %s exists", addr, id, hold)
	eventually(t, 30*time.Minute, "the hold file", func() bool {
		_, err := os.Stat(hold)
		return err == nil
	})
}

// screenText reads the session's screen through the portal.
func screenText(t *testing.T, pt *portal, id string) string {
	t.Helper()
	return screen(t, pt, domain.TaskID(id))
}

func pageServed(t *testing.T, pt *portal, snap snapshot) {
	for n, v := range snap.Versions {
		req, err := http.NewRequest(http.MethodGet, "http://"+pt.addr+strings.TrimPrefix(v.EntryURL, "http://"+pt.addr), nil)
		must(t, err)
		req.Header.Set(userHeader, "Mia")
		resp, err := http.DefaultClient.Do(req)
		must(t, err)
		b, _ := io.ReadAll(resp.Body)
		_ = resp.Body.Close()
		want := "<h1>Evidence v" + string(rune('1'+n)) + "</h1><p>An agent drove the app. 5 < 6 & ok.</p>"
		if resp.StatusCode != http.StatusOK || string(b) != want || !strings.Contains(resp.Header.Get("Content-Security-Policy"), "sandbox") {
			t.Errorf("version %d at %s: %s, %q, CSP %q", v.Version, v.EntryURL, resp.Status, b, resp.Header.Get("Content-Security-Policy"))
		}
	}
}

func reopened(t *testing.T, pt *portal, fakeURL, id string) {
	b, _ := json.Marshal(map[string]string{"text": "one more thing"})
	post(t, pt, "/api/tasks/"+id+"/input", b)
	var snap snapshot
	eventually(t, 2*time.Minute, "the reopened task done with a third version", func() bool {
		pt.call(t, "GET", "/api/tasks/"+id, nil, &snap)
		return snap.Task.State == "done" && len(snap.Versions) == 3
	})
	if c, e := fakeWrites(t, fakeURL); c != 1 || e != 1 {
		t.Fatalf("%d creates and %d edits, want 1 and 1", c, e)
	}
	comments := fakeComments(t, fakeURL)
	if len(comments) != 1 || !strings.Contains(comments[0].Body, "/results/"+id+"/3") {
		t.Fatalf("comments %+v", comments)
	}
	time.Sleep(3 * time.Second)
	pt.call(t, "GET", "/api/tasks/"+id, nil, &snap)
	if snap.Task.State != "done" {
		t.Errorf("after the harness exited the task is %s", snap.Task.State)
	}
}

// noTaskToken looks for the task token, which only the desk proxy holds, in the desk's environment, processes and
// files. The token goes to grep on stdin, so the search leaves no trace of its own.
func noTaskToken(t *testing.T, desk, deskProxy string) {
	env := mustRun(t, "docker", "inspect", "-f", "{{range .Config.Env}}{{println .}}{{end}}", deskProxy)
	token := ""
	for _, l := range strings.Split(env, "\n") {
		if v, ok := strings.CutPrefix(l, "MOOR_TASK_TOKEN="); ok {
			token = v
		}
	}
	if !strings.HasPrefix(token, "task.") {
		t.Fatalf("the desk proxy holds no task token:\n%s", env)
	}
	seen := mustRun(t, "docker", "exec", desk, "bash.real", "-c", "env; ps -eo args; cat /proc/[0-9]*/environ 2>/dev/null | tr '\\0' '\\n'")
	if strings.Contains(seen, token) || !strings.Contains(seen, "MOOR_TASK_TOKEN=moor-placeholder-task") {
		t.Fatalf("the desk's environment and processes hold the token, or lack the placeholder")
	}
	if n := strings.Count(scanDesk(t, desk, token), "\n"); n != 0 {
		t.Fatalf("%d files of the desk hold the token", n)
	}
	if got := scanDesk(t, desk, "Five of five flows passed."); !strings.Contains(got, "/desk/script") {
		t.Fatalf("the same search does not find a line the desk's files hold, so it proves nothing: %q", got)
	}
}

var denied = regexp.MustCompile(`(?m)^grep: (.+): Permission denied$`)

// scanDesk names the files of the desk that hold what. The desk drops every capability, so even its root cannot
// read some files: each of those is read from the host through docker cp.
func scanDesk(t *testing.T, desk, what string) string {
	t.Helper()
	cmd := exec.Command("docker", "exec", "-i", "-u", "0", desk, "grep", "-rlF", "-D", "skip", "-f", "-", "/", "--exclude-dir=proc", "--exclude-dir=sys", "--exclude-dir=dev")
	cmd.Stdin = strings.NewReader(what + "\n")
	var stdout, stderr strings.Builder
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	err := cmd.Run()
	var ee *exec.ExitError
	if err != nil && (!errors.As(err, &ee) || ee.ExitCode() > 2) {
		t.Fatalf("the file search did not run to an end: %v: %s", err, stderr.String())
	}
	if rest := strings.TrimSpace(denied.ReplaceAllString(stderr.String(), "")); rest != "" {
		t.Fatalf("the file search failed past unreadable files: %s", rest)
	}
	found := stdout.String()
	for _, m := range denied.FindAllStringSubmatch(stderr.String(), -1) {
		cp := exec.Command("sh", "-c", `docker cp "$0:$1" - | grep -caF -f -`, desk, m[1])
		cp.Stdin = strings.NewReader(what + "\n")
		out, _ := cp.Output()
		if n := strings.TrimSpace(string(out)); n != "0" {
			found += m[1] + " (read from the host)\n"
		}
	}
	return found
}

func personRefused(t *testing.T, pt *portal, id string) {
	req, err := http.NewRequest(http.MethodPost, "http://"+pt.addr+"/api/tasks/"+id+"/results", strings.NewReader(`{"line":"ok","page":"<p>x</p>","key":"k"}`))
	must(t, err)
	req.Header.Set(userHeader, "Mia")
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	must(t, err)
	_ = resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("a signed-in person publishing: %s", resp.Status)
	}
}
