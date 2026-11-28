package main

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/adapter/link"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/machine"
	"github.com/IniZio/moor/internal/settings"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/wire"
)

// runners serves the runner side of the portal: POST /runner/join and the link at GET /runner/link.
type runners struct {
	machines *machine.Service
	control  *machine.Control
	db       *store.Store
	hs       *link.Server
	holder   string
	log      *slog.Logger

	ctx    context.Context
	cancel context.CancelFunc
	wg     sync.WaitGroup
}

func newRunners(ctx context.Context, cfg config, db *store.Store, tasks *task.Service, svc *settings.Service) *runners {
	rn := &runners{db: db, holder: holderOf(cfg), log: slog.Default()}
	rn.ctx, rn.cancel = context.WithCancel(context.WithoutCancel(ctx))
	rn.machines = machine.New(db, tasks, systemClock{}, cfg.publicURL).WithRunnerArgs(cfg.runnerArgs)
	rn.control = machine.NewControl(machine.ControlConfig{Store: db, Tasks: tasks, Clock: systemClock{}, Access: newTokens(cfg.secretKey).scoped(cfg.publicURL), Tick: cfg.linkTick, Log: rn.log})
	if u, err := url.Parse(cfg.publicURL); err == nil && u.Host != "" {
		rn.hs, _ = link.NewServer(link.ServerConfig{Host: link.HostOf(u), Keys: keys{db}, Presence: presence{db, rn.holder}})
	}
	return rn
}

// holderOf names this replica in the machines it holds links for. With an internal address it is that
// address, which another replica dials for a terminal hop. Without one it is unique and not dialled.
func holderOf(cfg config) string {
	if cfg.internalURL != "" {
		return cfg.internalURL
	}
	host, _ := os.Hostname()
	return host + "-" + strings.ToLower(rand.Text()[:6])
}

// terms is the link control seen as the web's terminals.
type terms struct{ *machine.Control }

func (t terms) TermAttach(ctx context.Context, id domain.TaskID, a wire.TermAttach) (io.ReadWriteCloser, error) {
	st, err := t.Control.TermAttach(ctx, id, a)
	if err != nil {
		return nil, fmt.Errorf("terminal: %w", err)
	}
	return st, nil
}

// acts are the task actions of a Run page. A cancel also sends its stop at once over a link this
// replica holds, rather than on the holder's next tick.
type acts struct {
	*task.Service
	control *machine.Control
}

func (a acts) Cancel(ctx context.Context, id domain.TaskID) (task.Outcome, error) {
	out, err := a.Service.Cancel(ctx, id)
	if err == nil {
		a.control.Stop(context.WithoutCancel(ctx), out.Effects)
	}
	return out, err //nolint:wrapcheck // the web branches on the task service's own errors
}

// mount serves the runner routes beside next. Without a public URL no runner can sign a proof, so
// the runner routes are not served.
func (rn *runners) mount(next http.Handler) http.Handler {
	if rn.hs == nil {
		return next
	}
	r := chi.NewRouter()
	r.Post("/runner/join", rn.join)
	r.Get("/runner/link", rn.link)
	r.Mount("/", next)
	return r
}

// close ends every link this replica holds and waits for them.
func (rn *runners) close() {
	rn.cancel()
	rn.wg.Wait()
}

type joinRequest struct {
	ID        string `json:"id"`
	Token     string `json:"token"`
	Name      string `json:"name"`
	PublicKey []byte `json:"public_key"`
}

func (rn *runners) join(w http.ResponseWriter, r *http.Request) {
	var in joinRequest
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&in); err != nil {
		http.Error(w, "the body is not a join: id, token, name and public_key", http.StatusBadRequest)
		return
	}
	err := rn.machines.Join(r.Context(), machine.JoinInput{ID: domain.RunnerID(in.ID), Token: in.Token, Name: in.Name, PubKey: in.PublicKey})
	switch {
	case err == nil:
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"id": in.ID})
	case errors.Is(err, machine.ErrBadJoin):
		http.Error(w, err.Error(), http.StatusBadRequest)
	case errors.Is(err, domain.ErrJoinRefused):
		http.Error(w, "the join token is unknown, used or expired: add the machine again in the portal", http.StatusForbidden)
	case errors.Is(err, domain.ErrExists):
		http.Error(w, fmt.Sprintf("a machine named %q is already enrolled", in.Name), http.StatusConflict)
	default:
		rn.log.Error("join", "err", err)
		http.Error(w, "the join could not be stored, try again", http.StatusInternalServerError)
	}
}

// link accepts a runner's link, runs the handshake against the stored key and serves the link until
// it ends. The link lives on the replica's context, not the request's.
func (rn *runners) link(w http.ResponseWriter, r *http.Request) {
	c, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionDisabled})
	if err != nil {
		return
	}
	rn.wg.Add(1)
	defer rn.wg.Done()
	acc, err := rn.hs.Handshake(rn.ctx, c)
	if err != nil {
		rn.log.Info("link refused", "err", err)
		return
	}
	s, err := link.NewSession(rn.ctx, c, false)
	if err != nil {
		rn.log.Warn("link session", "machine", acc.Runner, "err", err)
		return
	}
	rn.log.Info("linked", "machine", acc.Runner, "gen", acc.Gen, "build", acc.Build)
	if err := rn.control.Serve(rn.ctx, acc.Runner, acc.Gen, session{s}); err != nil {
		rn.log.Info("link ended", "machine", acc.Runner, "err", err)
	}
	ctx, cancel := context.WithTimeout(context.WithoutCancel(rn.ctx), 5*time.Second)
	defer cancel()
	if err := rn.db.Unlinked(ctx, acc.Runner, acc.Gen); err != nil {
		rn.log.Warn("unlinked not recorded", "machine", acc.Runner, "err", err)
	}
}

type session struct{ *link.Session }

func (s session) Open(ctx context.Context, op wire.Kind, body, out any) (machine.Stream, error) {
	st, err := s.Session.Open(ctx, op, body, out)
	if err != nil {
		return nil, fmt.Errorf("link: %w", err)
	}
	return st, nil
}

type keys struct{ db *store.Store }

func (k keys) Lookup(ctx context.Context, id domain.RunnerID) (ed25519.PublicKey, error) {
	m, err := k.db.Machine(ctx, id)
	switch {
	case errors.Is(err, domain.ErrNotFound):
		return nil, fmt.Errorf("machine %s: %w", id, domain.ErrUnknownRunner)
	case err != nil:
		return nil, fmt.Errorf("machine %s: %w", id, err)
	case len(m.PubKey) == 0:
		return nil, fmt.Errorf("machine %s: %w", id, domain.ErrUnknownRunner)
	case !m.RevokedAt.IsZero():
		return m.PubKey, fmt.Errorf("machine %s: %w", id, domain.ErrRevokedRunner)
	}
	return m.PubKey, nil
}

type presence struct {
	db     *store.Store
	holder string
}

func (p presence) Linked(ctx context.Context, id domain.RunnerID, build string) (int64, error) {
	gen, err := p.db.Linked(ctx, id, build, p.holder, time.Now().UTC())
	if err != nil {
		return 0, fmt.Errorf("presence: %w", err)
	}
	return gen, nil
}

func (p presence) Unlinked(ctx context.Context, id domain.RunnerID, gen int64) error {
	if err := p.db.Unlinked(ctx, id, gen); err != nil {
		return fmt.Errorf("presence: %w", err)
	}
	return nil
}
