package link

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

type fakeStore struct {
	mu          sync.Mutex
	runner      domain.RunnerID
	key         ed25519.PublicKey
	lookupErr   error
	linkedErr   error
	linked      int
	unlinkedGen []int64
}

func (f *fakeStore) Lookup(_ context.Context, id domain.RunnerID) (ed25519.PublicKey, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	switch {
	case f.lookupErr != nil:
		return nil, f.lookupErr
	case id != f.runner:
		return nil, domain.ErrUnknownRunner
	}
	return f.key, nil
}

func (f *fakeStore) Linked(context.Context, domain.RunnerID, string) (int64, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.linkedErr != nil {
		return 0, f.linkedErr
	}
	f.linked++
	return int64(f.linked), nil
}

func (f *fakeStore) Unlinked(_ context.Context, _ domain.RunnerID, gen int64) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.unlinkedGen = append(f.unlinkedGen, gen)
	return nil
}

func (f *fakeStore) linkedCount() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.linked
}

type testServer struct {
	srv    *Server
	store  *fakeStore
	http   *httptest.Server
	key    ed25519.PrivateKey
	runner domain.RunnerID
	host   string
}

func (ts testServer) wsURL() string { return "ws" + ts.http.URL[len("http"):] }

func (ts testServer) agent() AgentConfig {
	return AgentConfig{URL: ts.wsURL(), Runner: ts.runner, Key: ts.key, Build: "b1", Timeout: time.Second}
}

func newTestServer(t *testing.T, edit func(*ServerConfig)) testServer {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	ts := testServer{store: &fakeStore{runner: "r1", key: pub}, key: priv, runner: "r1"}
	ts.http = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		c, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionDisabled})
		if err != nil {
			return
		}
		if _, err := ts.srv.Handshake(r.Context(), c); err == nil {
			_ = c.CloseNow()
		}
	}))
	t.Cleanup(ts.http.Close)
	u, _ := url.Parse(ts.http.URL)
	ts.host = HostOf(u)
	cfg := ServerConfig{Host: ts.host, Keys: ts.store, Presence: ts.store, Timeout: time.Second}
	if edit != nil {
		edit(&cfg)
	}
	if ts.srv, err = NewServer(cfg); err != nil {
		t.Fatal(err)
	}
	return ts
}

// rawConn is a runner written by hand, so a test can say anything the wire allows.
type rawConn struct {
	t *testing.T
	c *websocket.Conn
}

func dialRaw(t *testing.T, url string) rawConn {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	c, _, err := websocket.Dial(ctx, url, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.CloseNow() })
	return rawConn{t: t, c: c}
}

func (r rawConn) sendText(s string) {
	r.t.Helper()
	if err := r.c.Write(context.Background(), websocket.MessageText, []byte(s)); err != nil {
		r.t.Fatal(err)
	}
}

func (r rawConn) send(kind wire.Kind, body any) {
	r.t.Helper()
	raw, _ := json.Marshal(body)
	b, _ := json.Marshal(frame{Kind: kind, Body: raw})
	r.sendText(string(b))
}

func (r rawConn) recv() string {
	r.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_, data, err := r.c.Read(ctx)
	if err != nil {
		r.t.Fatalf("read: %v", err)
	}
	return string(data)
}

func (r rawConn) recvFrame() frame {
	r.t.Helper()
	var f frame
	if err := json.Unmarshal([]byte(r.recv()), &f); err != nil {
		r.t.Fatal(err)
	}
	return f
}

func (r rawConn) challenge(runner domain.RunnerID) []byte {
	r.t.Helper()
	r.send(wire.KindHello, wire.Hello{Runner: string(runner), Proto: wire.Proto, Build: "raw"})
	f := r.recvFrame()
	var ch wire.Challenge
	if err := f.decode(&ch); err != nil || f.Kind != wire.KindChallenge || len(ch.Nonce) != 32 {
		r.t.Fatalf("want a 32 byte challenge, got %+v", f)
	}
	return ch.Nonce
}

func (r rawConn) closeStatus() websocket.StatusCode {
	r.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_, _, err := r.c.Read(ctx)
	return websocket.CloseStatus(err)
}

func TestHandshakeAnswersUnknownRunnerAndBadProofAlike(t *testing.T) {
	ts := newTestServer(t, nil)
	_, other, _ := ed25519.GenerateKey(rand.Reader)
	cases := []struct {
		name   string
		runner domain.RunnerID
		key    ed25519.PrivateKey
	}{
		{"unknown runner", "ghost", ts.key},
		{"proof from another key", ts.runner, other},
	}
	var answers []string
	for _, c := range cases {
		rc := dialRaw(t, ts.wsURL())
		nonce := rc.challenge(c.runner)
		rc.send(wire.KindProof, wire.Proof{Sig: proof{ts.host, nonce, c.runner, wire.Proto}.sign(c.key)})
		answers = append(answers, rc.recv())
		if got := rc.closeStatus(); got != websocket.StatusNormalClosure {
			t.Errorf("%s: a refusal closes normally, got %d", c.name, got)
		}
	}
	if answers[0] != answers[1] || answers[0] != `{"kind":"refused","body":{"reason":"unauthorized"}}` {
		t.Fatalf("refusals must be identical so they are no oracle: %q", answers)
	}
	if ts.store.linkedCount() != 0 {
		t.Fatal("a refused hello must not be recorded as a link")
	}
}

func TestHandshakeRefusesAProofReplayedOnANewLink(t *testing.T) {
	ts := newTestServer(t, nil)
	first := dialRaw(t, ts.wsURL())
	n1 := first.challenge(ts.runner)
	sig := proof{ts.host, n1, ts.runner, wire.Proto}.sign(ts.key)
	first.send(wire.KindProof, wire.Proof{Sig: sig})
	if f := first.recvFrame(); f.Kind != wire.KindWelcome {
		t.Fatalf("the honest proof must be welcomed: %+v", f)
	}

	second := dialRaw(t, ts.wsURL())
	n2 := second.challenge(ts.runner)
	if string(n1) == string(n2) {
		t.Fatal("every connection gets a fresh nonce")
	}
	second.send(wire.KindProof, wire.Proof{Sig: sig})
	if f := second.recvFrame(); f.Kind != wire.KindRefused {
		t.Fatalf("a replayed proof must be refused: %+v", f)
	}
	if ts.store.linkedCount() != 1 {
		t.Fatal("only the honest link is recorded")
	}
}

func TestHandshakeAnswersUpdateRequiredForALowOrUnparsableHello(t *testing.T) {
	ts := newTestServer(t, nil)
	const frozen = `{"kind":"update_required","body":{"server_proto":1,"min_proto":1}}`
	cases := []struct {
		name string
		send func(rawConn)
	}{
		{"protocol below the floor", func(r rawConn) { r.send(wire.KindHello, wire.Hello{Runner: "r1", Proto: 0}) }},
		{"protocol from the future", func(r rawConn) { r.send(wire.KindHello, wire.Hello{Runner: "r1", Proto: 9}) }},
		{"not JSON", func(r rawConn) { r.sendText("hello?") }},
		{"JSON of another shape", func(r rawConn) { r.sendText(`["hello"]`) }},
		{"a hello with no body", func(r rawConn) { r.sendText(`{"kind":"hello"}`) }},
		{"another frame first", func(r rawConn) { r.send(wire.KindProof, wire.Proof{}) }},
		{"an unknown kind", func(r rawConn) { r.sendText(`{"kind":"hello2","body":{"runner":"r1","proto":1}}`) }},
		{"a binary frame", func(r rawConn) { _ = r.c.Write(context.Background(), websocket.MessageBinary, []byte{1, 2}) }},
	}
	for _, c := range cases {
		rc := dialRaw(t, ts.wsURL())
		c.send(rc)
		if got := rc.recv(); got != frozen {
			t.Errorf("%s: got %s", c.name, got)
		}
	}
}

func TestHandshakeHelloDecodingIsLenient(t *testing.T) {
	ts := newTestServer(t, nil)
	rc := dialRaw(t, ts.wsURL())
	rc.sendText(`{"kind":"hello","from_the_future":1,"body":{"runner":"r1","proto":1,"build":"x","also":{"a":[1,2]}}}`)
	if f := rc.recvFrame(); f.Kind != wire.KindChallenge {
		t.Fatalf("a newer runner's extra hello fields must be ignored: %+v", f)
	}
}

func TestHandshakeInfrastructureErrorsCloseWithoutAFrame(t *testing.T) {
	cases := map[string]func(*fakeStore){
		"key store down":        func(s *fakeStore) { s.lookupErr = errors.New("db down") },
		"presence down":         func(s *fakeStore) { s.linkedErr = errors.New("db down") },
		"stored key of no size": func(s *fakeStore) { s.key = ed25519.PublicKey{1, 2, 3} },
	}
	for name, fail := range cases {
		ts := newTestServer(t, nil)
		fail(ts.store)
		rc := dialRaw(t, ts.wsURL())
		rc.send(wire.KindHello, wire.Hello{Runner: string(ts.runner), Proto: wire.Proto})
		if name == "presence down" {
			var ch wire.Challenge
			_ = rc.recvFrame().decode(&ch)
			rc.send(wire.KindProof, wire.Proof{Sig: proof{ts.host, ch.Nonce, ts.runner, wire.Proto}.sign(ts.key)})
		}
		if got := rc.closeStatus(); got != websocket.StatusInternalError {
			t.Errorf("%s: want status 1011 and no frame, got %d", name, got)
		}
	}
}

func TestHandshakeSlowClientIsCutLoose(t *testing.T) {
	ts := newTestServer(t, func(c *ServerConfig) { c.Timeout = 50 * time.Millisecond })
	rc := dialRaw(t, ts.wsURL())
	start := time.Now()
	_ = rc.closeStatus()
	if time.Since(start) > 2*time.Second {
		t.Fatal("a client that never says hello must be dropped after the timeout")
	}
}

func TestDrainingServerAnswersRelink(t *testing.T) {
	ts := newTestServer(t, nil)
	ts.srv.Drain()
	rc := dialRaw(t, ts.wsURL())
	rc.send(wire.KindHello, wire.Hello{Runner: string(ts.runner), Proto: wire.Proto})
	if got := rc.recv(); got != `{"kind":"relink","body":{}}` {
		t.Fatalf("got %s", got)
	}
	if got := rc.closeStatus(); got != websocket.StatusNormalClosure {
		t.Fatalf("a relink closes normally, got %d", got)
	}
}

func TestNewServerNeedsItsParts(t *testing.T) {
	if _, err := NewServer(ServerConfig{Host: "h"}); err == nil {
		t.Fatal("a server without a key store and presence must not start")
	}
}
