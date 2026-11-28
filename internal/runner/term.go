package runner

import (
	"context"
	"errors"
	"fmt"

	"github.com/IniZio/moor/internal/wire"
)

// outChunk is the most terminal output one out frame carries.
const outChunk = 8 << 10

var errNoDesk = &wire.Error{Code: wire.CodeRefused, Msg: "the host has no desk for this task"}

// hasDesk refuses an op on a task the host holds none of.
func (r *Runner) hasDesk(task string) error {
	_, ok, err := r.held(r.ctx, task)
	switch {
	case err != nil:
		return err
	case !ok:
		return errNoDesk
	}
	return nil
}

// termAttach puts one viewer on the task's session and carries its frames until either end goes. Its
// own hc attach ends with it, the session does not. An observer's keys and size are dropped here too.
func (r *Runner) termAttach(st Stream) error {
	var a wire.TermAttach
	if err := decode(st, &a); err != nil {
		return err
	}
	if (a.Mode != wire.TermWrite && a.Mode != wire.TermObserve) || a.Cols == 0 || a.Rows == 0 {
		return errors.Join(errBadRequest, fmt.Errorf("mode %q at %dx%d", a.Mode, a.Cols, a.Rows))
	}
	if err := r.hasDesk(a.Task); err != nil {
		return err
	}
	ctx, cancel := context.WithCancel(r.ctx)
	defer cancel()
	tty, err := r.cfg.Sessions.Attach(ctx, a)
	if err != nil {
		return fmt.Errorf("runner: attach to the session of %s: %w", a.Task, err)
	}
	defer func() { _ = tty.Close() }()
	if err := reply(st, struct{}{}); err != nil {
		return err
	}
	go func() {
		keys(st, tty, a.Mode == wire.TermWrite)
		_ = tty.Close()
	}()
	screenOut(st, tty)
	return nil
}

// screenOut sends the viewer's terminal output until it ends, then an exit frame.
func screenOut(st Stream, tty TTY) {
	buf := make([]byte, outChunk)
	for {
		n, err := tty.Read(buf)
		if n > 0 && wire.WriteTerm(st, wire.TermOut, buf[:n]) != nil {
			return
		}
		if err != nil {
			_ = wire.WriteTerm(st, wire.TermExit, nil)
			return
		}
	}
}

// keys feeds in and resize frames to the viewer's terminal until the stream ends.
func keys(st Stream, tty TTY, write bool) {
	for {
		kind, b, err := wire.ReadTerm(st)
		if err != nil || (write && apply(tty, kind, b) != nil) {
			return
		}
	}
}

// apply passes one frame to the terminal. A bad size is dropped, a dead terminal ends the viewer.
func apply(tty TTY, kind wire.TermFrame, b []byte) error {
	switch kind {
	case wire.TermIn:
		_, err := tty.Write(b)
		return err //nolint:wrapcheck // the caller only stops on it
	case wire.TermResize:
		if s, err := wire.SizeOf(b); err == nil {
			_ = tty.Resize(s)
		}
	case wire.TermOut, wire.TermExit:
	}
	return nil
}

func (r *Runner) termRead(st Stream) error {
	var q wire.TermRead
	if err := decode(st, &q); err != nil {
		return err
	}
	if err := r.hasDesk(q.Task); err != nil {
		return err
	}
	text, err := r.cfg.Sessions.Screen(r.ctx, q.Task, q.Lines)
	if err != nil {
		return fmt.Errorf("runner: read the screen of %s: %w", q.Task, err)
	}
	return reply(st, wire.TermReadReply{Text: text})
}

func (r *Runner) termSend(st Stream) error {
	var q wire.TermSend
	if err := decode(st, &q); err != nil {
		return err
	}
	if err := r.hasDesk(q.Task); err != nil {
		return err
	}
	if err := r.cfg.Sessions.Type(r.ctx, q.Task, q.Text); err != nil {
		return fmt.Errorf("runner: type into the session of %s: %w", q.Task, err)
	}
	return reply(st, struct{}{})
}
