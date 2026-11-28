package e2e

import (
	"context"
	"errors"
	"net"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/adapter/store/pgtest"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// apiKey and ghKey are the synthetic values of the test's credentials. They are never printed: checks count them.
const (
	apiKey = "synthetic-key-4e1f0c9a7d"
	ghKey  = "synthetic-gh-0b7c55e1a2"
)

// accessDoc reaches GitHub's API on port 80, so the fake upstream answers it in plain HTTP. Its allow lists the
// REST merge, which the proxy refuses whatever the rules say.
const accessDoc = `repositories:
  - example-org/shop-web
credentials:
  shop:
    value: ` + apiKey + `
  github:
    value: ` + ghKey + `
rules:
  shop:
    hosts: ["shop-api.test:80", secure-api.test]
    credential: shop
    inject: {header: X-Api-Key, form: "{secret}"}
    allow: ["GET /hello", "GET /"]
    ask: ["DELETE /items/*"]
  github-api:
    hosts: ["api.github.com:80"]
    credential: github
    inject: {header: Authorization, form: "token {secret}"}
    allow: ["POST /graphql", "GET /repos/{repo}/**", "PUT /repos/{repo}/pulls/*/merge"]
    graphql_mutations: [createPullRequest]
    box_env: {GH_TOKEN: placeholder, GH_REPO: "{repo}"}
`

// gatewayAddr is a free address on the Docker bridge's gateway, which a task's proxies reach from their
// egress network, so they can read the policy from a portal on this host.
func gatewayAddr(t *testing.T) string {
	t.Helper()
	gw := strings.TrimSpace(mustRun(t, "docker", "network", "inspect", "bridge", "-f", "{{(index .IPAM.Config 0).Gateway}}"))
	ln, err := net.Listen("tcp", net.JoinHostPort(gw, "0"))
	must(t, err)
	defer func() { _ = ln.Close() }()
	return ln.Addr().String()
}

func mustRun(t *testing.T, name string, args ...string) string {
	t.Helper()
	out, err := runCmd(name, args...)
	if err != nil {
		t.Fatalf("%s %v: %v: %s", name, args, err, out)
	}
	return out
}

// upstream starts the fake upstream and joins it to the task's egress network under the hosts the test
// names, once the runner has made that network.
func upstream(t *testing.T, dir, prefix string, id domain.TaskID) string {
	t.Helper()
	bin := dir + "/upstream"
	cmd := exec.Command("go", "build", "-o", bin, "./testdata/upstream")
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build the upstream: %v: %s", err, out)
	}
	name := prefix + "-upstream"
	mustRun(t, "docker", "run", "-d", "--name", name, "--network", "none",
		"-v", bin+":/upstream:ro", "alpine:3.22@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f", "/upstream")
	t.Cleanup(func() { _, _ = runCmd("docker", "rm", "-f", name) })
	mustRun(t, "docker", "network", "disconnect", "none", name)
	mustRun(t, "docker", "network", "connect", "--alias", "shop-api.test", "--alias", "secure-api.test", "--alias", "later-api.test",
		"--alias", "api.github.com", prefix+"-"+string(id)+"-egress", name)
	return name
}

// inBox runs a shell line in the Box the way the agent does: bash -c in the desk, carried into the Box.
func inBox(t *testing.T, box, line string) string {
	t.Helper()
	desk := strings.TrimSuffix(box, "-box") + "-desk"
	out, _ := runCmd("docker", "exec", "-e", "BOX_SSH=rootless@"+box, desk, "bash", "-c", line)
	return strings.TrimSpace(out)
}

func TestTheBoxReachesOnlyWhatThePolicyAllowsAndNeverHoldsAValue(t *testing.T) {
	if os.Getenv(dockerEnv) != "1" {
		t.Skipf("%s is not 1: this test starts real containers, run it with make test-docker", dockerEnv)
	}
	dsn := pgtest.SchemaDSN(t)
	dir := t.TempDir()
	pt := &portal{bin: build(t, dir, "cmd/portal"), addr: gatewayAddr(t), dsn: dsn}
	runnerBin := build(t, dir, "cmd/runner")
	prefix := "moore2a-" + randHex(t, 4)
	t.Cleanup(func() {
		if left := leftBehind(t, prefix); len(left) > 0 {
			t.Errorf("left on the host: %v", left)
		}
	})
	pt.start(t)
	pt.call(t, "PUT", "/api/settings", map[string]any{"base": 0, "text": accessDoc}, nil)
	keyFile := dir + "/runner.key"
	mid := joined(t, pt, runnerBin, keyFile)
	start(t, "runner", nil, runnerBin, append([]string{"-portal", "ws://" + pt.addr + "/runner/link", "-id", mid, "-key", keyFile, "-prefix", prefix}, images...)...)
	eventually(t, time.Minute, "the machine linked", func() bool { return machineState(t, pt, mid) == "linked" })

	db, err := store.Open(context.Background(), dsn)
	must(t, err)
	defer db.Close()
	svc := task.New(db, clock{}, task.Config{})
	id, err := db.NextTaskID(context.Background())
	must(t, err)
	_, err = svc.StartByPerson(context.Background(), task.StartInput{ID: id, Repo: domain.Repo{Host: "github.com", Path: "example-org/shop-web"}, Prompt: "Call the shop API", By: "Mia"})
	must(t, err)
	eventually(t, 3*time.Minute, "the task running", taskIs(t, db, id, domain.Running))
	up := upstream(t, dir, prefix, id)
	box := prefix + "-" + string(id) + "-box"

	t.Run("an allowed host gets the connection value on the wire", func(t *testing.T) { valueOnTheWire(t, box, up) })
	if t.Failed() {
		out, _ := runCmd("docker", "logs", "--tail", "30", prefix+"-"+string(id)+"-boxproxy")
		t.Fatalf("Box egress proxy log:\n%s\ncurl:\n%s", out, inBox(t, box, "curl -sv http://shop-api.test/hello 2>&1 | grep -v -i x-api-key"))
	}
	t.Run("an intercepted host is trusted in the Box", func(t *testing.T) { intercepted(t, box) })
	t.Run("a path the rule does not list is refused", func(t *testing.T) { pathRefused(t, box) })
	t.Run("an ask entry is blocked, then allowed live", func(t *testing.T) { askedThenAllowed(t, pt, box, id) })
	t.Run("an unlisted host is refused and allowed without a restart", func(t *testing.T) { refusedThenAllowed(t, pt, box, id) })
	t.Run("a pull request passes and a merge never does", func(t *testing.T) { mergeRefused(t, pt, box, up, id) })
	t.Run("the Box holds no trace of any value", func(t *testing.T) { noTrace(t, box) })
	t.Run("the Box cannot open the desk proxy", func(t *testing.T) { deskProxyClosed(t, box, prefix+"-"+string(id)+"-deskproxy") })

	mustRun(t, "docker", "rm", "-f", up)
	_, err = svc.Cancel(context.Background(), id)
	must(t, err)
	eventually(t, time.Minute, "the cancelled task's containers removed", func() bool { return len(remaining(t, prefix+"-"+string(id))) == 0 })
}

func valueOnTheWire(t *testing.T, box, up string) {
	code := ""
	eventually(t, 30*time.Second, "the proxy reading the policy", func() bool {
		code = inBox(t, box, `curl -s -o /dev/null -w '%{http_code}' http://shop-api.test/hello`)
		return code == "200"
	})
	logs := mustRun(t, "docker", "logs", up)
	if n := strings.Count(logs, "host=shop-api.test key="+apiKey); n != 1 {
		t.Fatalf("the upstream saw the connection value on %d requests, want 1", n)
	}
}

// intercepted fetches a host with a connection over HTTPS. The proxy intercepts it and cannot verify the fake
// upstream's own certificate, so a 502 here means the Box trusted the proxy's certificate.
func intercepted(t *testing.T, box string) {
	if code := inBox(t, box, `curl -s -o /dev/null -w '%{http_code}' https://secure-api.test/`); code != "502" {
		t.Fatalf("an intercepted HTTPS request answered %q, want 502 from the proxy inside a trusted tunnel:\n%s", code, inBox(t, box, "curl -sv https://secure-api.test/ 2>&1 | tail -20"))
	}
}

type blockedRow struct {
	ID       int64  `json:"id"`
	Request  string `json:"request"`
	Reason   string `json:"reason"`
	Detail   string `json:"detail"`
	Decision string `json:"decision"`
}

// blockedRow waits for the task's blocked request that reads as request.
func waitBlocked(t *testing.T, pt *portal, id domain.TaskID, request string) blockedRow {
	t.Helper()
	var snap struct {
		Blocked []blockedRow `json:"blocked_requests"`
	}
	var row blockedRow
	eventually(t, 30*time.Second, "the blocked request "+request+" on the task", func() bool {
		pt.call(t, "GET", "/api/tasks/"+string(id), nil, &snap)
		for _, b := range snap.Blocked {
			if b.Request == request {
				row = b
				return true
			}
		}
		return false
	})
	return row
}

// allowLive chooses Allow on the row and waits for the next attempt of line in the Box to answer 200.
func allowLive(t *testing.T, pt *portal, box string, id domain.TaskID, row blockedRow, line string) {
	t.Helper()
	var allowed struct{ Line int }
	pt.call(t, "POST", "/api/tasks/"+string(id)+"/blocked/"+strconv.FormatInt(row.ID, 10), map[string]string{"action": "allow"}, &allowed)
	at := time.Now()
	eventually(t, 10*time.Second, "the next attempt passing", func() bool { return inBox(t, box, line) == "200" })
	t.Logf("Allow on %s added line %d; the next request passed %s later, with no restart", row.Request, allowed.Line, time.Since(at).Round(10*time.Millisecond))
}

func pathRefused(t *testing.T, box string) {
	out := inBox(t, box, `curl -s -D - http://shop-api.test/other`)
	if !strings.Contains(out, " 403 ") || !strings.Contains(strings.ToLower(out), "x-deny-reason: request_not_allowed") {
		t.Fatalf("an unlisted path answered:\n%s", out)
	}
}

func askedThenAllowed(t *testing.T, pt *portal, box string, id domain.TaskID) {
	line := `curl -s -o /dev/null -w '%{http_code}' -X DELETE http://shop-api.test/items/3`
	out := inBox(t, box, `curl -s -D - -X DELETE http://shop-api.test/items/3`)
	if !strings.Contains(out, " 403 ") || !strings.Contains(strings.ToLower(out), "x-deny-reason: needs_approval") {
		t.Fatalf("an ask entry answered:\n%s", out)
	}
	allowLive(t, pt, box, id, waitBlocked(t, pt, id, "shop-api.test:80 DELETE /items/3"), line)
}

func refusedThenAllowed(t *testing.T, pt *portal, box string, id domain.TaskID) {
	out := inBox(t, box, `curl -s -D - http://later-api.test/x`)
	if !strings.Contains(out, " 403 ") || !strings.Contains(strings.ToLower(out), "x-deny-reason: host_not_allowed") || !strings.Contains(out, "GET later-api.test:80") {
		t.Fatalf("the unlisted host answered:\n%s", out)
	}
	allowLive(t, pt, box, id, waitBlocked(t, pt, id, "later-api.test:80 GET /x"), `curl -s -o /dev/null -w '%{http_code}' http://later-api.test/x`)
}

// graphQL posts a GraphQL document from the Box with gh's token, the placeholder its environment holds.
func graphQL(t *testing.T, box, query string) string {
	t.Helper()
	body := strconv.Quote(query)
	return inBox(t, box, `curl -s -D - -X POST -H "Authorization: token $GH_TOKEN" -H 'Content-Type: application/json' `+
		`--data '{"query":`+body+`,"variables":{}}' http://api.github.com/graphql`)
}

// pullRequestPasses sends createPullRequest with the Box's placeholder token, and the upstream gets the value.
func pullRequestPasses(t *testing.T, box, up string) {
	t.Helper()
	if env := inBox(t, box, `echo "$GH_TOKEN $GH_REPO"`); env != "moor-placeholder-github-api example-org/shop-web" {
		t.Fatalf("the Box's gh variables are %q", env)
	}
	if out := graphQL(t, box, `mutation PullRequestCreate { createPullRequest(input: {}) { clientMutationId } }`); !strings.Contains(out, " 200 ") {
		t.Fatalf("createPullRequest answered:\n%s", out)
	}
	if n := strings.Count(mustRun(t, "docker", "logs", up), "auth=token "+ghKey+" method=POST path=/graphql"); n != 1 {
		t.Fatalf("the upstream saw the GitHub value on %d GraphQL requests, want 1", n)
	}
}

func mergeRefused(t *testing.T, pt *portal, box, up string, id domain.TaskID) {
	pullRequestPasses(t, box, up)
	merge := graphQL(t, box, `mutation PullRequestCreate { createPullRequest: mergePullRequest(input: {}) { clientMutationId } }`)
	rest := inBox(t, box, `curl -s -D - -X PUT -H "Authorization: token $GH_TOKEN" http://api.github.com/repos/example-org/shop-web/pulls/1/merge`)
	for name, out := range map[string]string{"the merge mutation": merge, "the REST merge": rest} {
		if !strings.Contains(out, " 403 ") || !strings.Contains(strings.ToLower(out), "x-deny-reason: merge_not_allowed") || !strings.Contains(out, "merging is not allowed from a task") {
			t.Fatalf("%s answered:\n%s", name, out)
		}
	}
	if !strings.Contains(merge, "mergePullRequest") {
		t.Fatalf("the merge refusal does not name the field:\n%s", merge)
	}
	row := waitBlocked(t, pt, id, "api.github.com:80 POST /graphql")
	if row.Reason != "merge_not_allowed" || !strings.Contains(row.Detail, "mergePullRequest") {
		t.Fatalf("the merge's blocked request is %+v", row)
	}
	if n := strings.Count(mustRun(t, "docker", "logs", up), "path=/repos/example-org/shop-web/pulls/1/merge"); n != 0 {
		t.Fatalf("the REST merge reached the upstream %d times", n)
	}
}

// noTrace counts the values in the Box's environment, its processes and its files. The values go to grep on
// stdin, so the search leaves no trace of its own.
func noTrace(t *testing.T, box string) {
	seen := inBox(t, box, `env; ps -eo args; cat /proc/[0-9]*/environ 2>/dev/null | tr '\0' '\n'`)
	if n := strings.Count(seen, apiKey) + strings.Count(seen, ghKey); n != 0 || !strings.Contains(seen, "HTTPS_PROXY") || !strings.Contains(seen, "GH_TOKEN=moor-placeholder") {
		t.Fatalf("the values show %d times in the Box's environment and processes", n)
	}
	cmd := exec.Command("docker", "exec", "-i", "-u", "0", box, "grep", "-rlsF", "-D", "skip", "-f", "-", "/", "--exclude-dir=proc", "--exclude-dir=sys", "--exclude-dir=dev")
	cmd.Stdin = strings.NewReader(apiKey + "\n" + ghKey + "\n")
	out, err := cmd.CombinedOutput()
	if files := strings.Fields(string(out)); len(files) != 0 {
		t.Fatalf("%d files in the Box hold the value: %v", len(files), files)
	}
	var ee *exec.ExitError
	if err == nil || !errors.As(err, &ee) || ee.ExitCode() != 1 {
		t.Fatalf("the file search did not run to a clean no-match: %v", err)
	}
	control := exec.Command("docker", "exec", "-i", "-u", "0", box, "grep", "-rlsF", "-D", "skip", "-f", "-", "/", "--exclude-dir=proc", "--exclude-dir=sys", "--exclude-dir=dev")
	control.Stdin = strings.NewReader("nsenter -U --preserve-credentials\n")
	if out, err := control.CombinedOutput(); err != nil || !strings.Contains(string(out), "ns-shell") {
		t.Fatalf("the same search does not find a string the Box's files hold, so it proves nothing: %v: %s", err, out)
	}
}

func deskProxyClosed(t *testing.T, box, deskProxy string) {
	ips := strings.Fields(mustRun(t, "docker", "inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}", deskProxy))
	if len(ips) != 2 {
		t.Fatalf("desk proxy addresses %v", ips)
	}
	for _, ip := range ips {
		if out, err := runCmd("docker", "exec", box, "curl", "-sS", "-m", "3", "--noproxy", "*", "http://"+ip+":8080/"); err == nil {
			t.Errorf("the Box opened the desk proxy at %s: %s", ip, out)
		}
	}
}
