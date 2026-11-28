package e2e

import (
	"bytes"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

const hookSecret = "e2e-hook-secret"

const reviewedEvidenceSpec = `name: PR evidence
on:
  pull_request:
    types: [opened]
concurrency:
  cancel-in-progress: true
prompt: Show evidence for ${{ event.pull_request.html_url }} at ${{ event.pull_request.head.sha }}.
review: Check your page against the evidence skill's check list. Fix what fails, then publish again.
`

// homeWith writes a team home that holds the spec and returns its folder.
func homeWith(t *testing.T, dir string) string {
	t.Helper()
	home := filepath.Join(dir, "home")
	if err := os.MkdirAll(filepath.Join(home, "tasks"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "tasks", "pr-evidence.yaml"), []byte(reviewedEvidenceSpec), 0o600); err != nil {
		t.Fatal(err)
	}
	return home
}

var fakeLine = regexp.MustCompile(`(?m)^(url|key)=(.+)$`)

// fakeGitHub runs the fake GitHub as a process and returns its address and the throwaway App key it made.
func fakeGitHub(t *testing.T, dir string) (url, key string) {
	t.Helper()
	p := start(t, "fakegithub", nil, build(t, dir, "cmd/fakegithub"), "-key-dir", t.TempDir())
	got := map[string]string{}
	eventually(t, 30*time.Second, "the fake GitHub's address", func() bool {
		b, _ := os.ReadFile(p.log)
		for _, m := range fakeLine.FindAllStringSubmatch(string(b), -1) {
			got[m[1]] = m[2]
		}
		return got["url"] != "" && got["key"] != ""
	})
	return got["url"], got["key"]
}

// fakeWrites counts what the fake GitHub was asked to do to comments.
func fakeWrites(t *testing.T, url string) (creates, edits int) {
	t.Helper()
	var calls []struct{ Method, Path string }
	getJSON(t, url+"/_fake/calls", &calls)
	for _, c := range calls {
		switch {
		case c.Method == http.MethodPost && strings.HasSuffix(c.Path, "/issues/90/comments"):
			creates++
		case c.Method == http.MethodPatch && strings.Contains(c.Path, "/issues/comments/"):
			edits++
		}
	}
	return creates, edits
}

func fakeComments(t *testing.T, url string) []struct {
	ID   int64
	Body string
} {
	t.Helper()
	var out []struct {
		ID   int64
		Body string
	}
	getJSON(t, url+"/_fake/comments", &out)
	return out
}

func getJSON(t *testing.T, url string, out any) {
	t.Helper()
	resp, err := http.Get(url)
	must(t, err)
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(resp.Body)
	if err := json.Unmarshal(b, out); err != nil {
		t.Fatalf("%s: %v in %s", url, err, b)
	}
}

// openedPullRequest sends GitHub's signed pull_request delivery for pull request 90 at head to the portal and returns
// the id of the task it started.
func openedPullRequest(t *testing.T, pt *portal, head string) string {
	t.Helper()
	repo := "example-org/shop-web"
	body, _ := json.Marshal(map[string]any{
		"action": "opened", "number": 90,
		"repository": map[string]any{"full_name": repo, "name": "shop-web", "owner": map[string]any{"login": "example-org"}, "html_url": "https://github.com/" + repo},
		"sender":     map[string]any{"login": "ana"},
		"pull_request": map[string]any{
			"number": 90, "title": "Add search to the shop", "state": "open", "html_url": "https://github.com/" + repo + "/pull/90",
			"user": map[string]any{"login": "ana"},
			"head": map[string]any{"sha": head, "ref": "search", "repo": map[string]any{"full_name": repo}},
			"base": map[string]any{"ref": "main"},
		},
	})
	mac := hmac.New(sha256.New, []byte(hookSecret))
	mac.Write(body)
	req, err := http.NewRequest(http.MethodPost, "http://"+pt.addr+"/hooks/github", bytes.NewReader(body))
	must(t, err)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-GitHub-Event", "pull_request")
	req.Header.Set("X-GitHub-Delivery", "e2e-"+randHex(t, 4))
	req.Header.Set("X-Hub-Signature-256", "sha256="+hex.EncodeToString(mac.Sum(nil)))
	resp, err := http.DefaultClient.Do(req)
	must(t, err)
	defer func() { _ = resp.Body.Close() }()
	var out struct {
		Tasks []string `json:"tasks"`
	}
	b, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusAccepted || json.Unmarshal(b, &out) != nil || len(out.Tasks) != 1 {
		t.Fatalf("the webhook answered %s: %s", resp.Status, b)
	}
	return out.Tasks[0]
}

// deskWrite puts a file in the task's desk, where the stand-in harness reads its script and its pages.
func deskWrite(t *testing.T, desk, path, content string) {
	t.Helper()
	cmd := exec.Command("docker", "exec", "-i", desk, "bash.real", "-c", "cat > "+path)
	cmd.Stdin = strings.NewReader(content)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("write %s in the desk: %v: %s", path, err, out)
	}
}
