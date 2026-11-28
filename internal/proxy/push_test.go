package proxy_test

import (
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"testing/iotest"
	"time"

	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp/capability"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp/sideband"

	"github.com/IniZio/moor/internal/wire"
)

const (
	refusedDefault = "pushing to the default branch is not allowed from a task"
	refusedDelete  = "deleting a branch is not allowed from a task"
	refusedTag     = "pushing tags is not allowed from a task"
)

var (
	oldID = plumbing.NewHash("1111111111111111111111111111111111111111")
	newID = plumbing.NewHash("2222222222222222222222222222222222222222")
	// pack stands in for the pack that follows the commands. The proxy never reads it.
	pack = []byte("PACK\x00\x00\x00\x02\x00\x00\x00\x01synthetic pack bytes")
)

func create(ref string) *packp.Command {
	return &packp.Command{Name: plumbing.ReferenceName(ref), Old: plumbing.ZeroHash, New: newID}
}

func update(ref string) *packp.Command {
	return &packp.Command{Name: plumbing.ReferenceName(ref), Old: oldID, New: newID}
}

func remove(ref string) *packp.Command {
	return &packp.Command{Name: plumbing.ReferenceName(ref), Old: oldID, New: plumbing.ZeroHash}
}

// receivePack is a receive-pack body as git sends it: the commands, the first with the capabilities, a flush, then the pack.
func receivePack(t *testing.T, caps []capability.Capability, cmds ...*packp.Command) []byte {
	t.Helper()
	req := packp.NewReferenceUpdateRequest()
	for _, c := range caps {
		if err := req.Capabilities.Set(c); err != nil {
			t.Fatal(err)
		}
	}
	req.Commands = cmds
	var b bytes.Buffer
	if err := req.Encode(&b); err != nil {
		t.Fatal(err)
	}
	return append(b.Bytes(), pack...)
}

var gitCaps = []capability.Capability{capability.ReportStatus, capability.Sideband64k, capability.OFSDelta}

// gitServer records the bodies of the pushes that reach it.
type gitServer struct {
	mu       sync.Mutex
	bodies   [][]byte
	encoding []string
	srv      *httptest.Server
}

func newGitServer(t *testing.T) *gitServer {
	g := &gitServer{}
	g.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		g.mu.Lock()
		g.bodies, g.encoding = append(g.bodies, b), append(g.encoding, r.Header.Get("Content-Encoding"))
		g.mu.Unlock()
		_, _ = io.WriteString(w, "pushed")
	}))
	t.Cleanup(g.srv.Close)
	return g
}

func (g *gitServer) got() ([][]byte, []string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	return append([][]byte(nil), g.bodies...), append([]string(nil), g.encoding...)
}

func pushRig(t *testing.T, defaultBranch string) (*rig, *gitServer) {
	t.Helper()
	up := newGitServer(t)
	g := newRig(t, wire.Policy{})
	pol := policy(1, rule("git", localhost(up.srv), "GET /**", "POST /**"))
	pol.DefaultBranch = defaultBranch
	g.portal.set(pol)
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	return g, up
}

func (g *rig) push(t *testing.T, up *gitServer, body io.Reader, header http.Header) (*http.Response, []byte) {
	t.Helper()
	req, err := http.NewRequest(http.MethodPost, "http://"+localhost(up.srv)+"/example-org/shop-web.git/git-receive-pack", body)
	if err != nil {
		t.Fatal(err)
	}
	req.Header = header
	req.Header.Set("Content-Type", "application/x-git-receive-pack-request")
	resp, err := g.client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(resp.Body)
	return resp, b
}

func gzipped(t *testing.T, b []byte) []byte {
	var out bytes.Buffer
	z := gzip.NewWriter(&out)
	_, _ = z.Write(b)
	if err := z.Close(); err != nil {
		t.Fatal(err)
	}
	return out.Bytes()
}

// statuses reads the report-status git shows for each ref, from inside side-band 1 when the push asked for it.
func statuses(t *testing.T, body []byte, banded bool) map[string]string {
	t.Helper()
	var r io.Reader = bytes.NewReader(body)
	if banded {
		r = sideband.NewDemuxer(sideband.Sideband64k, r)
	}
	rs := packp.NewReportStatus()
	if err := rs.Decode(r); err != nil {
		t.Fatalf("the report-status does not decode: %v: %q", err, body)
	}
	out := map[string]string{}
	for _, s := range rs.CommandStatuses {
		out[s.ReferenceName.String()] = s.Status
	}
	return out
}

func TestAPushIsJudgedByItsCommandsAndTheRestStreamsUntouched(t *testing.T) {
	feature := receivePack(t, gitCaps, create("refs/heads/fix-checkout"))
	cases := []struct {
		name string
		body []byte
		gzip bool
		want map[string]string
	}{
		{name: "a new feature branch", body: feature},
		{name: "an update of a feature branch", body: receivePack(t, gitCaps, update("refs/heads/fix-checkout"))},
		{name: "the default branch", body: receivePack(t, gitCaps, update("refs/heads/trunk")), want: map[string]string{"refs/heads/trunk": refusedDefault}},
		{name: "the default branch in other case", body: receivePack(t, gitCaps, update("refs/heads/Trunk")), want: map[string]string{"refs/heads/Trunk": refusedDefault}},
		{name: "a branch delete", body: receivePack(t, gitCaps, remove("refs/heads/old")), want: map[string]string{"refs/heads/old": refusedDelete}},
		{name: "a new tag", body: receivePack(t, gitCaps, create("refs/tags/v1.0.0")), want: map[string]string{"refs/tags/v1.0.0": refusedTag}},
		{name: "a tag delete", body: receivePack(t, gitCaps, remove("refs/tags/v1.0.0")), want: map[string]string{"refs/tags/v1.0.0": refusedTag}},
		{
			name: "several refs with one bad", body: receivePack(t, gitCaps, create("refs/heads/a"), update("refs/tags/v2"), create("refs/heads/b")),
			want: map[string]string{"refs/heads/a": "another ref of this push is not allowed from a task", "refs/tags/v2": refusedTag, "refs/heads/b": "another ref of this push is not allowed from a task"},
		},
		{name: "a gzip feature branch", body: feature, gzip: true},
		{
			name: "report-status-v2, as git asks", body: receivePack(t, []capability.Capability{"report-status-v2", capability.Sideband64k}, create("refs/tags/v4")),
			want: map[string]string{"refs/tags/v4": refusedTag},
		},
		{name: "a gzip tag", body: receivePack(t, gitCaps, create("refs/tags/v3")), gzip: true, want: map[string]string{"refs/tags/v3": refusedTag}},
	}
	g, up := pushRig(t, "trunk")
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) { g.judged(t, up, c.body, c.gzip, c.want) })
	}
}

// judged pushes body and checks it reached the upstream byte for byte when want is empty, or was refused with
// want's status for each ref and reached nothing.
func (g *rig) judged(t *testing.T, up *gitServer, plain []byte, zipped bool, want map[string]string) {
	t.Helper()
	before, _ := up.got()
	body, header := plain, http.Header{}
	if zipped {
		body = gzipped(t, plain)
		header.Set("Content-Encoding", "gzip")
	}
	resp, got := g.push(t, up, bytes.NewReader(body), header)
	after, enc := up.got()
	if want == nil {
		passedWhole(t, resp, got, after[len(before):], enc[len(before):], plain)
		return
	}
	if len(after) != len(before) || resp.StatusCode != http.StatusOK || resp.Header.Get("X-Deny-Reason") != "push_not_allowed" {
		t.Fatalf("a refused push: %s, reason %q, %d reached the upstream", resp.Status, resp.Header.Get("X-Deny-Reason"), len(after)-len(before))
	}
	if s := statuses(t, got, true); !equalMaps(s, want) {
		t.Errorf("statuses %v, want %v", s, want)
	}
}

// passedWhole checks one push reached the upstream as it was sent, with no encoding.
func passedWhole(t *testing.T, resp *http.Response, got []byte, bodies [][]byte, enc []string, plain []byte) {
	t.Helper()
	if resp.StatusCode != http.StatusOK || string(got) != "pushed" || len(bodies) != 1 {
		t.Fatalf("%s: %q, %d reached the upstream", resp.Status, got, len(bodies))
	}
	if !bytes.Equal(bodies[0], plain) || enc[0] != "" {
		t.Fatalf("the upstream got %d bytes (encoding %q), want the %d bytes pushed", len(bodies[0]), enc[0], len(plain))
	}
}

func equalMaps(a, b map[string]string) bool {
	if len(a) != len(b) {
		return false
	}
	for k, v := range a {
		if b[k] != v {
			return false
		}
	}
	return true
}

func TestARefusedPushIsReportedOncePerReasonWithoutAnAllowEntry(t *testing.T) {
	g, up := pushRig(t, "trunk")
	g.push(t, up, bytes.NewReader(receivePack(t, gitCaps, update("refs/heads/trunk"), create("refs/tags/v1"))), http.Header{})
	waitFor(t, "two reports", func() bool { return len(g.portal.reports()) == 2 })
	details := map[string]bool{}
	for _, r := range g.portal.reports() {
		if r.Reason != "push_not_allowed" || r.Entry != "" || r.Rule != "git" || r.Method != "POST" || r.Path != "/example-org/shop-web.git/git-receive-pack" {
			t.Errorf("report %+v", r)
		}
		details[r.Detail] = true
	}
	if !details[refusedDefault] || !details[refusedTag] {
		t.Errorf("details %v", details)
	}
}

func TestAPushWithoutReportStatusIsRefusedWith403(t *testing.T) {
	g, up := pushRig(t, "trunk")
	resp, body := g.push(t, up, bytes.NewReader(receivePack(t, nil, create("refs/tags/v1"))), http.Header{})
	if resp.StatusCode != http.StatusForbidden || resp.Header.Get("X-Deny-Reason") != "push_not_allowed" || !strings.Contains(string(body), refusedTag) {
		t.Fatalf("%s %q: %s", resp.Status, resp.Header.Get("X-Deny-Reason"), body)
	}
}

func TestAPushWhoseCommandsCannotBeReadIsRefused(t *testing.T) {
	g, up := pushRig(t, "trunk")
	for name, body := range map[string][]byte{
		"garbage":           []byte("this is not a pkt-line stream"),
		"a cut command":     receivePack(t, gitCaps, create("refs/heads/a"))[:30],
		"no flush":          bytes.TrimSuffix(receivePack(t, gitCaps, create("refs/heads/a")), append([]byte("0000"), pack...)),
		"gzip of garbage":   gzipped(t, []byte("0032not a command at all")),
		"an empty body":     nil,
		"a flush then more": append([]byte("0000"), pack...),
	} {
		header := http.Header{}
		if strings.HasPrefix(name, "gzip") {
			header.Set("Content-Encoding", "gzip")
		}
		resp, got := g.push(t, up, bytes.NewReader(body), header)
		if resp.StatusCode != http.StatusForbidden || resp.Header.Get("X-Deny-Reason") != "push_not_allowed" {
			t.Errorf("%s: %s %q: %s", name, resp.Status, resp.Header.Get("X-Deny-Reason"), got)
		}
	}
	if bodies, _ := up.got(); len(bodies) != 0 {
		t.Errorf("%d refused pushes reached the upstream", len(bodies))
	}
}

func TestGitsProbeOfAnEmptyFlushPasses(t *testing.T) {
	g, up := pushRig(t, "trunk")
	if resp, got := g.push(t, up, strings.NewReader("0000"), http.Header{}); resp.StatusCode != http.StatusOK || string(got) != "pushed" {
		t.Fatalf("the probe: %s %q", resp.Status, got)
	}
}

func TestABodySplitAcrossReadsIsJudgedTheSame(t *testing.T) {
	g, up := pushRig(t, "trunk")
	body := receivePack(t, gitCaps, create("refs/heads/a"), create("refs/heads/b"))
	resp, _ := g.push(t, up, io.NopCloser(iotest.OneByteReader(bytes.NewReader(body))), http.Header{})
	bodies, _ := up.got()
	if resp.StatusCode != http.StatusOK || len(bodies) != 1 || !bytes.Equal(bodies[0], body) {
		t.Fatalf("%s, %d bodies", resp.Status, len(bodies))
	}
	resp, got := g.push(t, up, io.NopCloser(iotest.OneByteReader(bytes.NewReader(receivePack(t, gitCaps, remove("refs/heads/a"))))), http.Header{})
	if resp.StatusCode != http.StatusOK || statuses(t, got, true)["refs/heads/a"] != refusedDelete {
		t.Fatalf("%s %q", resp.Status, got)
	}
}

func TestWithTheDefaultBranchUnknownMainAndMasterAreRefused(t *testing.T) {
	g, up := pushRig(t, "")
	unread := refusedDefault + ": the default branch could not be read, so main and master are refused"
	for _, ref := range []string{"refs/heads/main", "refs/heads/master"} {
		resp, got := g.push(t, up, bytes.NewReader(receivePack(t, gitCaps, update(ref))), http.Header{})
		if s := statuses(t, got, true)[ref]; resp.StatusCode != http.StatusOK || s != unread {
			t.Errorf("%s: %s %q", ref, resp.Status, s)
		}
	}
	if resp, got := g.push(t, up, bytes.NewReader(receivePack(t, gitCaps, update("refs/heads/trunk"))), http.Header{}); string(got) != "pushed" {
		t.Errorf("a branch beside main and master: %s %q", resp.Status, got)
	}
}

// summer is an upstream that hashes the body it reads and says when the first bytes of it arrived.
type summer struct {
	first chan struct{}
	once  sync.Once
	mu    sync.Mutex
	sum   []byte
}

func (s *summer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	h := sha256.New()
	_, _ = io.Copy(io.MultiWriter(h, writerFunc(func(p []byte) {
		if len(p) > 0 {
			s.once.Do(func() { close(s.first) })
		}
	})), r.Body)
	s.mu.Lock()
	s.sum = h.Sum(nil)
	s.mu.Unlock()
	_, _ = io.WriteString(w, "pushed")
}

type writerFunc func([]byte)

func (f writerFunc) Write(p []byte) (int, error) { f(p); return len(p), nil }

// TestALargePushStreams holds back the rest of a 32 MiB pack until the upstream has seen its start, which only a
// proxy that streams the body can deliver.
func TestALargePushStreams(t *testing.T) {
	up := &summer{first: make(chan struct{})}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	g := newRig(t, wire.Policy{})
	g.portal.set(policy(1, rule("git", localhost(srv), "POST /**")))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	pr, pw := io.Pipe()
	want, streamed := sha256.New(), make(chan bool, 1)
	go func() {
		head, chunk := receivePack(t, gitCaps, create("refs/heads/big")), bytes.Repeat([]byte("synthetic-pack-data-"), 1<<15)
		_, _ = io.MultiWriter(want, pw).Write(head)
		select {
		case <-up.first:
			streamed <- true
		case <-time.After(10 * time.Second):
			streamed <- false
		}
		for range 48 {
			_, _ = io.MultiWriter(want, pw).Write(chunk)
		}
		_ = pw.Close()
	}()
	req, _ := http.NewRequest(http.MethodPost, "http://"+localhost(srv)+"/example-org/shop-web.git/git-receive-pack", pr)
	resp, err := g.client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(resp.Body)
	_ = resp.Body.Close()
	up.mu.Lock()
	defer up.mu.Unlock()
	if resp.StatusCode != http.StatusOK || string(b) != "pushed" || !bytes.Equal(up.sum, want.Sum(nil)) || !<-streamed {
		t.Fatalf("%s %q: the upstream got other bytes than pushed, or saw none before the rest was sent", resp.Status, b)
	}
}
