package machine

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/wire"
)

// HolderFresh is how recently another replica must have heard the runner to be dialled for a terminal.
const HolderFresh = 60 * time.Second

// TermHolder says where the task's terminal is served: here, or at the holder another replica named
// when it took the machine's link. A task with no desk, or whose machine no replica heard lately, has
// no live session.
func (c *Control) TermHolder(ctx context.Context, id domain.TaskID) (string, bool, error) {
	t, err := c.deskOf(ctx, id)
	if err != nil {
		return "", false, err
	}
	if _, ok := c.held(t.Runner); ok {
		return "", true, nil
	}
	m, err := c.cfg.Store.Machine(ctx, t.Runner)
	if err != nil {
		return "", false, fmt.Errorf("terminal of %s: %w", id, err)
	}
	if m.Holder == "" || c.cfg.Clock.Now().Sub(m.HeardAt) > HolderFresh {
		return "", false, fmt.Errorf("terminal of %s: its machine is not linked: %w", id, domain.ErrNoSession)
	}
	return m.Holder, false, nil
}

func (c *Control) deskOf(ctx context.Context, id domain.TaskID) (domain.Task, error) {
	t, err := c.cfg.Store.Load(ctx, id)
	switch {
	case err != nil:
		return domain.Task{}, fmt.Errorf("terminal of %s: %w", id, err)
	case t.Runner == "" || t.State == domain.Waiting || !keeps(t, c.cfg.Clock.Now()):
		return domain.Task{}, fmt.Errorf("terminal of %s: %w", id, domain.ErrNoSession)
	}
	return t, nil
}

// open opens op on the link of the task's machine, which this replica must hold. A runner that says it
// has no desk for the task means there is no session.
func (c *Control) open(ctx context.Context, id domain.TaskID, op wire.Kind, body, out any) (Stream, error) {
	t, err := c.deskOf(ctx, id)
	if err != nil {
		return nil, err
	}
	h, ok := c.held(t.Runner)
	if !ok {
		return nil, fmt.Errorf("terminal of %s: the link moved off this replica: %w", id, domain.ErrNoSession)
	}
	st, err := h.link.Open(ctx, op, body, out)
	if errors.Is(err, &wire.Error{Code: wire.CodeRefused}) {
		return nil, fmt.Errorf("%s of %s: %w: %w", op, id, domain.ErrNoSession, err)
	}
	if err != nil {
		return nil, fmt.Errorf("%s of %s: %w", op, id, err)
	}
	return st, nil
}

// TermAttach opens a viewer's byte stream on the task's session. Closing the stream ends the viewer.
func (c *Control) TermAttach(ctx context.Context, id domain.TaskID, a wire.TermAttach) (Stream, error) {
	a.Task = string(id)
	return c.open(ctx, id, wire.KindTermAttach, a, nil)
}

// Screen reads the session's screen as text, with lines of scrollback above it.
func (c *Control) Screen(ctx context.Context, id domain.TaskID, lines int) (string, error) {
	var out wire.TermReadReply
	st, err := c.open(ctx, id, wire.KindTermRead, wire.TermRead{Task: string(id), Lines: lines}, &out)
	if err != nil {
		return "", err
	}
	_ = st.Close()
	return out.Text, nil
}

// Type types text into the session and presses enter.
func (c *Control) Type(ctx context.Context, id domain.TaskID, text string) error {
	st, err := c.open(ctx, id, wire.KindTermSend, wire.TermSend{Task: string(id), Text: text}, nil)
	if err != nil {
		return err
	}
	_ = st.Close()
	return nil
}

// Stop sends the stop effects of an outcome at once over the links this replica holds. A link held
// elsewhere gets the stop on its holder's next tick, which also repeats any stop sent here.
func (c *Control) Stop(ctx context.Context, effects []task.Effect) {
	for _, e := range effects {
		h, ok := c.held(e.Runner)
		if e.Kind != task.StopJob || !ok {
			continue
		}
		f := wire.Fence{Task: string(e.Task), Run: e.Run, Gen: e.Gen}
		if err := call(ctx, h.link, wire.KindTaskStop, wire.TaskStop{Fence: f}, nil); err != nil {
			h.log.Info("stop not sent, the next tick sends it", "task", e.Task, "err", err)
		}
	}
}
