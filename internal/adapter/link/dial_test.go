package link

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/coder/websocket"

	"github.com/IniZio/moor/internal/wire"
)

func dialCtx(t *testing.T) context.Context {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	return ctx
}

// scriptedServer answers the runner's hello with reply and nothing else.
func scriptedServer(t *testing.T, reply string) string {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		c, err := websocket.Accept(w, r, nil)
		if err != nil {
			return
		}
		defer func() { _ = c.CloseNow() }()
		if _, _, err := c.Read(r.Context()); err == nil {
			_ = c.Write(r.Context(), websocket.MessageText, []byte(reply))
		}
	}))
	t.Cleanup(srv.Close)
	return "ws" + srv.URL[len("http"):]
}

func TestDialIsWelcomedWithTheStoredGeneration(t *testing.T) {
	ts := newTestServer(t, nil)
	for want := int64(1); want <= 2; want++ {
		c, w, err := Dial(dialCtx(t), ts.agent())
		if err != nil {
			t.Fatal(err)
		}
		_ = c.CloseNow()
		if w.Gen != want || w.Proto != wire.Proto {
			t.Fatalf("welcome %+v, want gen %d: every link is a later generation", w, want)
		}
	}
}

func TestDialTellsRefusalFromUpdateFromRelink(t *testing.T) {
	ts := newTestServer(t, nil)

	_, other, _ := ed25519.GenerateKey(rand.Reader)
	bad := ts.agent()
	bad.Key = other
	var refused *RefusedError
	if _, _, err := Dial(dialCtx(t), bad); !errors.As(err, &refused) || refused.Reason != refusedReason {
		t.Fatalf("a proof from another key is a refusal: %v", err)
	}

	future := ts.agent()
	future.Proto = wire.Proto + 8
	var update *UpdateRequiredError
	if _, _, err := Dial(dialCtx(t), future); !errors.As(err, &update) || update.ServerProto != wire.Proto || update.MinProto != 1 {
		t.Fatalf("a protocol from the future is update-required with both versions: %v", err)
	}

	ts.srv.Drain()
	if _, _, err := Dial(dialCtx(t), ts.agent()); !errors.Is(err, ErrRelinkLater) {
		t.Fatalf("a draining server asks for a relink: %v", err)
	}
}

func TestDialReadsWhatTheServerSaysOutOfOrder(t *testing.T) {
	cases := []struct {
		name  string
		reply string
		want  func(error) bool
	}{
		{"a kind from a newer server", `{"kind":"welcome2"}`, func(err error) bool {
			var u *UpdateRequiredError
			return errors.As(err, &u)
		}},
		{"a welcome before the challenge", `{"kind":"welcome","body":{"proto":1,"gen":1}}`, func(err error) bool {
			return errors.Is(err, errProtocol)
		}},
		{"a reply that is not JSON", "nope", func(err error) bool { return errors.Is(err, errBadFrame) }},
	}
	for _, c := range cases {
		cfg := AgentConfig{URL: scriptedServer(t, c.reply), Runner: "r1", Timeout: time.Second}
		if _, _, err := Dial(dialCtx(t), cfg); !c.want(err) {
			t.Errorf("%s: %v", c.name, err)
		}
	}
}

func TestDialRejectsABadURL(t *testing.T) {
	if _, _, err := Dial(dialCtx(t), AgentConfig{URL: "not a url"}); err == nil {
		t.Fatal("a URL with no host must fail before any dial")
	}
}
