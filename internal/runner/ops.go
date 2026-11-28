package runner

import (
	"context"
	"errors"
	"fmt"

	"github.com/IniZio/moor/internal/wire"
)

// errBadRequest marks a request body the runner could not read.
var errBadRequest = errors.New("runner: unreadable request")

type handler func(r *Runner, st Stream) error

// ops are the portal's ops this runner serves. Any other op is answered with unknown_op.
var ops = map[wire.Kind]handler{
	wire.KindTaskStart:    (*Runner).start,
	wire.KindTaskStop:     (*Runner).stopTask,
	wire.KindTaskRelease:  (*Runner).release,
	wire.KindTaskList:     (*Runner).listOp,
	wire.KindTaskWatch:    (*Runner).watchOp,
	wire.KindRunnerStatus: (*Runner).status,
	wire.KindTermAttach:   (*Runner).termAttach,
	wire.KindTermRead:     (*Runner).termRead,
	wire.KindTermSend:     (*Runner).termSend,
}

// serve runs one op. Ops run on the runner's context, not the link's: a link that drops mid-op
// leaves the op to finish, so the portal's repeat joins it rather than finding it cut short.
func (r *Runner) serve(st Stream) {
	defer func() { _ = st.Close() }()
	h, ok := ops[st.Op()]
	if !ok {
		_ = st.Fail(&wire.Error{Code: wire.CodeUnknownOp, Msg: string(st.Op())})
		return
	}
	if err := h(r, st); err != nil {
		r.cfg.Log.Warn("op failed", "op", st.Op(), "err", err)
		_ = st.Fail(asWire(err))
	}
}

func asWire(err error) *wire.Error {
	var we *wire.Error
	switch {
	case errors.As(err, &we):
		return we
	case errors.Is(err, errBadRequest):
		return &wire.Error{Code: wire.CodeBadRequest, Msg: err.Error()}
	case errors.Is(err, context.Canceled):
		return &wire.Error{Code: wire.CodeCancelled, Msg: err.Error()}
	}
	return &wire.Error{Code: wire.CodeInternal, Msg: err.Error()}
}

func reply(st Stream, body any) error {
	if err := st.Reply(body); err != nil {
		return fmt.Errorf("runner: reply to %s: %w", st.Op(), err)
	}
	return nil
}

func decode(st Stream, v any) error {
	if err := st.Decode(v); err != nil {
		return errors.Join(errBadRequest, err)
	}
	return nil
}

// start brings the task's parts up and its session live. An equal fence joins: the parts are
// already up, so Up changes nothing and the live session is kept. A newer fence replaces what
// differs. An older fence, or a fence the host has a newer launch for, is refused.
func (r *Runner) start(st Stream) error {
	var s wire.TaskStart
	if err := decode(st, &s); err != nil {
		return err
	}
	j := r.job(s.Task)
	j.op.Lock()
	defer j.op.Unlock()
	h, ok, err := r.held(r.ctx, s.Task)
	if err != nil {
		return err
	}
	if ok && wire.Admit(&h.Fence, s.Fence) == wire.Refuse {
		return &wire.Error{Code: wire.CodeRefused, Msg: "the host runs a newer launch of this task"}
	}
	if err := r.cfg.Host.Up(r.ctx, Box{Fence: s.Fence, Proxies: s.Proxies, Env: s.BoxEnv}); err != nil {
		return fmt.Errorf("runner: up %s: %w", s.Task, err)
	}
	if err := r.cfg.Sessions.Start(r.ctx, s); err != nil {
		return fmt.Errorf("runner: start the session of %s: %w", s.Task, err)
	}
	r.watch(s.Fence)
	r.note(j, s.Fence)
	if err := r.emitParts(s.Task); err != nil {
		return err
	}
	return reply(st, struct{}{})
}

// stopTask removes the task's containers. A stop with an older fence than the host's is a no-op,
// and so is a stop for a task the host does not have.
func (r *Runner) stopTask(st Stream) error {
	var s wire.TaskStop
	if err := decode(st, &s); err != nil {
		return err
	}
	return r.down(st, s.Task, func(h Held) bool { return !wire.Newer(h.Fence, s.Fence) })
}

// release removes the task's containers whatever their fence.
func (r *Runner) release(st Stream) error {
	var s wire.TaskRelease
	if err := decode(st, &s); err != nil {
		return err
	}
	return r.down(st, s.Task, func(Held) bool { return true })
}

func (r *Runner) down(st Stream, task string, ok func(Held) bool) error {
	j := r.job(task)
	j.op.Lock()
	defer j.op.Unlock()
	h, found, err := r.held(r.ctx, task)
	if err != nil {
		return err
	}
	if found && ok(h) {
		r.unwatch(task)
		if err := r.cfg.Host.Down(r.ctx, h.Fence); err != nil {
			return fmt.Errorf("runner: down %s: %w", task, err)
		}
		for _, p := range h.Parts {
			r.hub.emit(wire.TaskWatchLine{Fence: h.Fence, Part: p.Name, State: Removed})
		}
	}
	return reply(st, struct{}{})
}

// Removed is the state of a part in a task.watch line once the runner took it down.
const Removed = "removed"

func (r *Runner) listOp(st Stream) error {
	if err := decode(st, &wire.TaskList{}); err != nil {
		return err
	}
	held, err := r.list(r.ctx)
	if err != nil {
		return err
	}
	out := wire.TaskListReply{Tasks: make([]wire.ListedTask, 0, len(held))}
	for _, h := range held {
		lt := wire.ListedTask{Fence: h.Fence}
		for _, p := range h.Parts {
			lt.Containers = append(lt.Containers, p.Container)
		}
		out.Tasks = append(out.Tasks, lt)
	}
	return reply(st, out)
}

func (r *Runner) status(st Stream) error {
	if err := decode(st, &wire.RunnerStatus{}); err != nil {
		return err
	}
	held, err := r.list(r.ctx)
	if err != nil {
		return err
	}
	return reply(st, wire.RunnerStatusReply{Build: r.cfg.Build, Proto: wire.Proto, Slots: r.cfg.Slots, Tasks: len(held)})
}
