package machine

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// ControlConfig wires the control of runner links. Access gives a task's proxies the portal's address and
// their tokens, sent when the task starts, BoxEnv the variables the settings set in its Box, and DeskEnv those they
// set in its harness session on the desk, if any. Zero Tick takes the default.
type ControlConfig struct {
	Store   Store
	Tasks   Tasks
	Clock   Clock
	Access  func(t domain.Task) wire.ProxyAccess
	BoxEnv  func(ctx context.Context, t domain.Task) []wire.EnvVar
	DeskEnv func(ctx context.Context, t domain.Task) []wire.EnvVar
	Tick    time.Duration
	Log     *slog.Logger
}

const defaultTick = 2 * time.Second

// Control drives the tasks on the runner links this replica holds. It keeps nothing a restart
// would lose: every decision is read again from the store on each tick.
// The links it holds are kept by machine only so that terminals and stops can reach them.
type Control struct {
	cfg   ControlConfig
	mu    sync.Mutex
	links map[domain.RunnerID]*holding
}

func NewControl(cfg ControlConfig) *Control {
	if cfg.Tick == 0 {
		cfg.Tick = defaultTick
	}
	if cfg.Log == nil {
		cfg.Log = slog.New(slog.DiscardHandler)
	}
	return &Control{cfg: cfg, links: map[domain.RunnerID]*holding{}}
}

// hold names h as the machine's link on this replica until the returned func lets it go.
func (c *Control) hold(h *holding) func() {
	c.mu.Lock()
	c.links[h.id] = h
	c.mu.Unlock()
	return func() {
		c.mu.Lock()
		defer c.mu.Unlock()
		if c.links[h.id] == h {
			delete(c.links, h.id)
		}
	}
}

// held is the machine's link if this replica holds it.
func (c *Control) held(id domain.RunnerID) (*holding, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	h, ok := c.links[id]
	return h, ok
}

// holding is one link this replica holds, from its handshake to its end.
type holding struct {
	c     *Control
	id    domain.RunnerID
	gen   int64
	link  Link
	slots int
	log   *slog.Logger

	wg       sync.WaitGroup
	mu       sync.Mutex
	starting map[domain.TaskID]bool
}

// Serve drives the machine's tasks over l until the link ends, ctx ends, or a newer link, a
// disconnect or a revoke takes the link's generation. It closes l on the way out.
func (c *Control) Serve(ctx context.Context, id domain.RunnerID, gen int64, l Link) error {
	ctx, cancel := context.WithCancel(ctx)
	h := &holding{c: c, id: id, gen: gen, link: l, log: c.cfg.Log.With("machine", id, "gen", gen), starting: map[domain.TaskID]bool{}}
	defer func() {
		cancel()
		_ = l.Close()
		h.wg.Wait()
	}()
	var st wire.RunnerStatusReply
	if err := call(ctx, l, wire.KindRunnerStatus, wire.RunnerStatus{}, &st); err != nil {
		return err
	}
	h.slots = st.Slots
	defer c.hold(h)()
	h.wg.Go(func() { h.watch(ctx) })
	tick := time.NewTicker(c.cfg.Tick)
	defer tick.Stop()
	for h.tick(ctx) {
		select {
		case <-l.Done():
			return nil
		case <-ctx.Done():
			return nil
		case <-tick.C:
		}
	}
	return nil
}

// tick keeps the link only while its generation is the newest, then settles what the runner holds
// and offers it waiting tasks. A store error skips the tick and keeps the link.
func (h *holding) tick(ctx context.Context) bool {
	ok, err := h.c.cfg.Store.Heard(ctx, h.id, h.gen, h.c.cfg.Clock.Now())
	switch {
	case err != nil:
		h.log.Warn("heard not recorded", "err", err)
		return true
	case !ok:
		h.log.Info("link ended: a newer link, a disconnect or a revoke took it")
		return false
	}
	if err := h.reconcile(ctx); err != nil {
		h.log.Warn("reconcile", "err", err)
	}
	if err := h.place(ctx); err != nil {
		h.log.Warn("place", "err", err)
	}
	if err := h.reviews(ctx); err != nil {
		h.log.Warn("review", "err", err)
	}
	return true
}

func call(ctx context.Context, l Link, op wire.Kind, body, out any) error {
	st, err := l.Open(ctx, op, body, out)
	if err != nil {
		return fmt.Errorf("%s: %w", op, err)
	}
	_ = st.Close()
	return nil
}
