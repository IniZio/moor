package link_test

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/adapter/link/linktest"
	"github.com/IniZio/moor/internal/wire"
)

// linked dials the fake portal and returns both ends of one link.
func linked(t *testing.T) (portal, runner *link.Session, ctx context.Context) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	t.Cleanup(cancel)
	p := linktest.New(t)
	c, _, err := link.Dial(ctx, p.Agent())
	if err != nil {
		t.Fatal(err)
	}
	runner, err = link.NewSession(ctx, c, true)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = runner.Close() })
	portal, err = p.Next(ctx)
	if err != nil {
		t.Fatal(err)
	}
	return portal, runner, ctx
}

func TestAnOpCarriesItsBodyAndItsReply(t *testing.T) {
	portal, runner, ctx := linked(t)
	go func() {
		st, err := runner.Accept(ctx)
		if err != nil {
			return
		}
		defer func() { _ = st.Close() }()
		var stop wire.TaskStop
		if st.Op() != wire.KindTaskStop || st.Decode(&stop) != nil {
			_ = st.Fail(&wire.Error{Code: wire.CodeBadRequest, Msg: "want task.stop"})
			return
		}
		_ = st.Reply(wire.TaskListReply{Tasks: []wire.ListedTask{{Fence: stop.Fence}}})
	}()
	var got wire.TaskListReply
	st, err := portal.Open(ctx, wire.KindTaskStop, wire.TaskStop{Fence: wire.Fence{Task: "t1", Run: 2, Gen: 3}}, &got)
	if err != nil {
		t.Fatal(err)
	}
	_ = st.Close()
	if len(got.Tasks) != 1 || got.Tasks[0].Fence != (wire.Fence{Task: "t1", Run: 2, Gen: 3}) {
		t.Fatalf("reply %+v", got)
	}
}

func TestAFailedOpReturnsItsCode(t *testing.T) {
	portal, runner, ctx := linked(t)
	go func() {
		st, err := runner.Accept(ctx)
		if err != nil {
			return
		}
		var stop wire.TaskStop
		err = st.Decode(&stop)
		_ = st.Fail(&wire.Error{Code: wire.CodeBadRequest, Msg: err.Error()})
		_ = st.Close()
	}()
	_, err := portal.Open(ctx, wire.KindTaskStop, map[string]any{"task": "t1", "extra": 1}, nil)
	if !errors.Is(err, &wire.Error{Code: wire.CodeBadRequest}) {
		t.Fatalf("a request with an unknown field is refused: %v", err)
	}
}

func TestFramesFlowAfterTheReplyUntilTheOpenerLeaves(t *testing.T) {
	portal, runner, ctx := linked(t)
	served := make(chan error, 1)
	go func() { served <- sendLines(ctx, runner) }()
	watch, cancel := context.WithCancel(ctx)
	st, err := portal.Open(watch, wire.KindTaskWatch, wire.TaskWatch{}, nil)
	if err != nil {
		t.Fatal(err)
	}
	for want := int64(1); want <= 3; want++ {
		var line wire.TaskWatchLine
		if err := st.Recv(&line); err != nil || line.Run != want {
			t.Fatalf("line %d: %+v, %v", want, line, err)
		}
	}
	cancel()
	select {
	case err := <-served:
		if err == nil {
			t.Fatal("the sender stops with an error")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("a sender that reads its stream sees the opener leave")
	}
}

// sendLines answers one stream and sends numbered lines on it until the opener leaves, which it
// learns by reading the stream.
func sendLines(ctx context.Context, runner *link.Session) error {
	st, err := runner.Accept(ctx)
	if err != nil {
		return fmt.Errorf("accept: %w", err)
	}
	_ = st.Reply(struct{}{})
	go func() {
		var none struct{}
		_ = st.Recv(&none)
		_ = st.Close()
	}()
	for i := int64(1); ; i++ {
		if err := st.Send(wire.TaskWatchLine{Fence: wire.Fence{Task: "t1", Run: i}}); err != nil {
			return fmt.Errorf("send: %w", err)
		}
		time.Sleep(10 * time.Millisecond)
	}
}
