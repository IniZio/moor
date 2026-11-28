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

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

const peerSecret = "e2e-replicas-share-this"

// replicas are two portals on one database that hop terminals to each other.
func replicas(t *testing.T, dsn, bin string) (*portal, *portal) {
	t.Helper()
	var two [2]*portal
	for i := range two {
		addr := freeAddr(t)
		two[i] = &portal{bin: bin, addr: addr, dsn: dsn, env: []string{"PORTAL_INTERNAL_URL=http://" + addr, "PORTAL_INTERNAL_SECRET=" + peerSecret}}
		two[i].start(t)
	}
	return two[0], two[1]
}

// viewer is a browser's terminal connection: what it has printed so far, and its keys.
type viewer struct {
	c   *websocket.Conn
	out chan string
}

func attachTerm(t *testing.T, pt *portal, id domain.TaskID, person, mode string) *viewer {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	h := http.Header{userHeader: {person}, "Origin": {"http://" + pt.addr}}
	c, _, err := websocket.Dial(ctx, "ws://"+pt.addr+"/api/tasks/"+string(id)+"/terminal?mode="+mode+"&cols=100&rows=30", &websocket.DialOptions{HTTPHeader: h})
	must(t, err)
	t.Cleanup(func() { _ = c.CloseNow() })
	v := &viewer{c: c, out: make(chan string, 256)}
	go func() {
		defer close(v.out)
		for {
			typ, b, err := c.Read(context.Background())
			if err != nil {
				return
			}
			if typ == websocket.MessageBinary {
				v.out <- string(b)
			}
		}
	}()
	return v
}

func (v *viewer) sees(t *testing.T, want string) {
	t.Helper()
	var all strings.Builder
	deadline := time.After(time.Minute)
	for !strings.Contains(all.String(), want) {
		select {
		case b, ok := <-v.out:
			if !ok {
				t.Fatalf("the terminal closed before %q, after %q", want, all.String())
			}
			all.WriteString(b)
		case <-deadline:
			t.Fatalf("no %q on the terminal, which printed %q", want, all.String())
		}
	}
}

func (v *viewer) types(t *testing.T, keys string) {
	t.Helper()
	must(t, v.c.Write(context.Background(), websocket.MessageBinary, []byte(keys)))
}

func screen(t *testing.T, pt *portal, id domain.TaskID) string {
	t.Helper()
	req, err := http.NewRequest(http.MethodGet, "http://"+pt.addr+"/api/tasks/"+string(id)+"/screen?lines=100", nil)
	must(t, err)
	req.Header.Set(userHeader, "Ana")
	resp, err := http.DefaultClient.Do(req)
	must(t, err)
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("screen: %s: %s", resp.Status, b)
	}
	return string(b)
}

func viewersInDesk(t *testing.T, prefix string, id domain.TaskID) int {
	t.Helper()
	out, err := runCmd("docker", "exec", prefix+"-"+string(id)+"-desk", "ps", "-eo", "args")
	must(t, err)
	return strings.Count(out, "hc attach")
}

// runningOn starts a task as Mia on a runner linked to pt and waits for it to run.
func runningOn(t *testing.T, pt *portal, runnerBin, dir, prefix string) (domain.TaskID, *task.Service) {
	t.Helper()
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": settingsDoc}, nil)
	keyFile := dir + "/runner.key"
	id := joined(t, pt, runnerBin, keyFile)
	start(t, "runner", nil, runnerBin, append([]string{
		"-portal", "ws://" + pt.addr + "/runner/link", "-id", id,
		"-key", keyFile, "-prefix", prefix, "-desk-allow", "api.example.com",
	}, images...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, id) == "linked" })
	db, err := store.Open(context.Background(), pt.dsn)
	must(t, err)
	t.Cleanup(db.Close)
	svc := task.New(db, clock{}, task.Config{})
	tid, err := db.NextTaskID(context.Background())
	must(t, err)
	_, err = svc.StartByPerson(context.Background(), task.StartInput{ID: tid, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: "Fix the checkout", By: "Mia"})
	must(t, err)
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, tid, domain.Running))
	return tid, svc
}

func TestABrowserOnOneReplicaTypesIntoASessionLinkedToTheOther(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dir := t.TempDir()
	prefix := "moore2e-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	linked, viewing := replicas(t, pgtest.SchemaDSN(t), build(t, dir, "cmd/portal"))
	id, svc := runningOn(t, linked, build(t, dir, "cmd/runner"), dir, prefix)

	mia := attachTerm(t, viewing, id, "Mia", "steer")
	mia.types(t, "hello-term\r")
	mia.sees(t, "standin heard: hello-term")
	if got := screen(t, viewing, id); !strings.Contains(got, "standin heard: hello-term") {
		t.Fatalf("the screen route shows %q", got)
	}
	ana := attachTerm(t, linked, id, "Ana", "watch")
	ana.sees(t, "standin heard: hello-term")
	mia.types(t, "second-line\r")
	ana.sees(t, "standin heard: second-line")

	b, _ := json.Marshal(map[string]string{"text": "from-the-line"})
	post(t, viewing, "/api/tasks/"+string(id)+"/input", b)
	mia.sees(t, "standin heard: from-the-line")

	_ = mia.c.Close(websocket.StatusNormalClosure, "")
	_ = ana.c.Close(websocket.StatusNormalClosure, "")
	eventually(t, time.Minute, "the viewers' hc attach gone from the desk", func() bool { return viewersInDesk(t, prefix, id) == 0 })
	if got := screen(t, linked, id); !strings.Contains(got, "standin heard: from-the-line") {
		t.Fatalf("after the viewers left the session shows %q", got)
	}
	_, err := svc.Cancel(context.Background(), id)
	must(t, err)
	eventually(t, time.Minute, "the cancelled task's containers, networks and volumes removed", func() bool { return len(remaining(t, prefix)) == 0 })
}

func post(t *testing.T, pt *portal, path string, body []byte) {
	t.Helper()
	req, err := http.NewRequest(http.MethodPost, "http://"+pt.addr+path, strings.NewReader(string(body)))
	must(t, err)
	req.Header.Set(userHeader, "Mia")
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	must(t, err)
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusNoContent {
		b, _ := io.ReadAll(resp.Body)
		t.Fatalf("POST %s: %s: %s", path, resp.Status, b)
	}
}
