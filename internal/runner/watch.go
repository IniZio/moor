package runner

import (
	"context"
	"sync"
	"time"

	"github.com/cenkalti/backoff/v5"

	"github.com/IniZio/moor/internal/wire"
)

// watchBuffer is how many lines a task.watch stream may fall behind before it is closed. The portal
// reopens it and gets the levels again, so a slow watcher costs a resend, never a lost edge.
const watchBuffer = 256

// hub fans task.watch lines out to every open watch stream. During a portal deploy two replicas
// may each hold one.
type hub struct {
	mu   sync.Mutex
	subs map[chan wire.TaskWatchLine]struct{}
}

func (h *hub) sub() (<-chan wire.TaskWatchLine, func()) {
	c := make(chan wire.TaskWatchLine, watchBuffer)
	h.mu.Lock()
	h.subs[c] = struct{}{}
	h.mu.Unlock()
	return c, func() { h.drop(c) }
}

func (h *hub) drop(c chan wire.TaskWatchLine) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, ok := h.subs[c]; ok {
		delete(h.subs, c)
		close(c)
	}
}

func (h *hub) emit(l wire.TaskWatchLine) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for c := range h.subs {
		select {
		case c <- l:
		default:
			delete(h.subs, c)
			close(c)
		}
	}
}

// watchOp serves task.watch: the level of every part and session first, then each edge, until the
// portal closes the stream. The stream subscribes before it reads the levels, so an edge in between
// arrives twice rather than never.
func (r *Runner) watchOp(st Stream) error {
	if err := decode(st, &wire.TaskWatch{}); err != nil {
		return err
	}
	lines, unsub := r.hub.sub()
	defer unsub()
	levels, err := r.levels("")
	if err != nil {
		return err
	}
	if err := reply(st, struct{}{}); err != nil {
		return err
	}
	gone := make(chan struct{})
	go func() {
		var none struct{}
		_ = st.Recv(&none)
		close(gone)
	}()
	for _, l := range levels {
		if st.Send(l) != nil {
			return nil
		}
	}
	return r.edges(st, lines, gone)
}

func (r *Runner) edges(st Stream, lines <-chan wire.TaskWatchLine, gone <-chan struct{}) error {
	for {
		select {
		case l, ok := <-lines:
			if !ok || st.Send(l) != nil {
				return nil
			}
		case <-gone:
			return nil
		case <-r.ctx.Done():
			return nil
		}
	}
}

// levels is a line for each part the host shows and each session last heard, for one task or,
// with task empty, for all.
func (r *Runner) levels(task string) ([]wire.TaskWatchLine, error) {
	held, err := r.list(r.ctx)
	if err != nil {
		return nil, err
	}
	var out []wire.TaskWatchLine
	for _, h := range held {
		if task != "" && h.Fence.Task != task {
			continue
		}
		for _, p := range h.Parts {
			out = append(out, wire.TaskWatchLine{Fence: h.Fence, Part: p.Name, State: p.State})
		}
		r.mu.Lock()
		if j := r.jobs[h.Fence.Task]; j != nil && j.session != nil {
			out = append(out, *j.session)
		}
		r.mu.Unlock()
	}
	return out, nil
}

func (r *Runner) emitParts(task string) error {
	levels, err := r.levels(task)
	if err != nil {
		return err
	}
	for _, l := range levels {
		r.hub.emit(l)
	}
	return nil
}

// watch follows the session of the launch f, replacing the follower of an older launch.
func (r *Runner) watch(f wire.Fence) {
	j := r.job(f.Task)
	r.mu.Lock()
	defer r.mu.Unlock()
	if j.stop != nil && j.watching == f {
		return
	}
	if j.stop != nil {
		j.stop()
	}
	ctx, cancel := context.WithCancel(r.ctx)
	j.watching, j.stop = f, cancel
	r.wg.Go(func() { r.follow(ctx, j, f) })
}

// follow forwards the session's states as task.watch lines, reconnecting with backoff when the
// desk cannot be reached, until its launch is replaced or removed.
func (r *Runner) follow(ctx context.Context, j *job, f wire.Fence) {
	on := func(s SessionState) {
		l := wire.TaskWatchLine{Fence: f, Part: SessionPart, State: s.State, Code: s.Code}
		r.mu.Lock()
		j.session = &l
		r.mu.Unlock()
		r.hub.emit(l)
	}
	b := backoff.NewExponentialBackOff()
	b.MaxInterval = 5 * time.Second
	_, _ = backoff.Retry(ctx, func() (struct{}, error) { return struct{}{}, r.cfg.Sessions.Watch(ctx, f.Task, on) },
		backoff.WithBackOff(b), backoff.WithMaxElapsedTime(0))
}

func (r *Runner) unwatch(task string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if j := r.jobs[task]; j != nil && j.stop != nil {
		j.stop()
		j.stop, j.session = nil, nil
	}
}

func (r *Runner) unwatchAll() {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, j := range r.jobs {
		if j.stop != nil {
			j.stop()
			j.stop = nil
		}
	}
}
