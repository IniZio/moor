package e2e

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
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
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn, env: []string{
		"PORTAL_HOME_DIR=" + homeWith(t, dir), "PORTAL_GITHUB_APP_ID=17", "PORTAL_GITHUB_PRIVATE_KEY_FILE=" + keyFile,
		"PORTAL_GITHUB_API_URL=" + fakeURL + "/", "PORTAL_GITHUB_WEBHOOK_SECRET=" + hookSecret,
	}}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moore9a-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": settingsDoc}, nil)
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
	for n, name := range []string{"page1", "page2", "page3"} {
		deskWrite(t, desk, "/desk/"+name+".html", "<h1>Evidence v"+string(rune('1'+n))+"</h1><p>An agent drove the app. 5 < 6 & ok.</p>")
	}
	deskWrite(t, desk, "/desk/script", script)
	deskWrite(t, desk, "/desk/go", "")

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
	holdForABrowser(t, pt.addr, id)
	left := leftBehind(t, prefix)
	t.Logf("the done task keeps its desk and Box for the keep window, which nothing ends yet, so the test removed %d containers, %d networks and %d volumes", len(left["container"]), len(left["network"]), len(left["volume"]))
	if again := remaining(t, prefix); len(again) != 0 {
		t.Errorf("still on the host after the removal: %v", again)
	}
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
