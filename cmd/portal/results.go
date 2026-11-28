package main

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"strconv"
	"strings"

	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/answer"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
	"github.com/IniZio/moor/internal/web"
	"github.com/IniZio/moor/internal/wire"
)

// taskScope is the kind of the token a task's desk presents to publish. It is minted the way a proxy's token is, under the
// same key, so any replica checks it with no table. A proxy refuses it, since it is no proxy's kind, and this
// route refuses a proxy's token, so neither opens the other's routes.
const taskScope domain.ProxyKind = "task"

// scoped is access with the task scope's token added, which the runner hands to the desk.
func (t tokens) scoped(portal string) func(domain.Task) wire.ProxyAccess {
	base := t.access(portal)
	return func(task domain.Task) wire.ProxyAccess {
		a := base(task)
		a.TaskToken = t.mint(task.ID, taskScope)
		return a
	}
}

func (t tokens) verifyTask(id domain.TaskID, token string) bool {
	kind, sig, _ := strings.Cut(token, ".")
	got, err := base64.RawURLEncoding.DecodeString(sig)
	return err == nil && domain.ProxyKind(kind) == taskScope && hmac.Equal(got, t.sum(id, taskScope))
}

// results takes the pages a desk publishes. The page is kept in the database with its version and served by the
// portal itself, and a task that answers an event posts its answer in the same call.
type results struct {
	tokens  tokens
	answers *answer.Service
	db      *store.Store
	public  string
}

func newResults(cfg config, db *store.Store, answers *answer.Service) results {
	return results{tokens: newTokens(cfg.secretKey), answers: answers, db: db, public: strings.TrimRight(cfg.publicURL, "/")}
}

func (r results) Publish(ctx context.Context, id domain.TaskID, token string, in web.ResultInput) (web.ResultReceipt, error) {
	if !r.tokens.verifyTask(id, token) {
		return web.ResultReceipt{}, domain.ErrBadToken
	}
	page := []byte(in.Page)
	out, err := r.answers.Publish(ctx, id, task.PublishInput{
		Line: strings.TrimSpace(in.Line), EntryURL: r.public + "/results/" + string(id) + "/" + task.EntryURLVersion,
		Digest: digest(in), Page: page,
	})
	if err != nil {
		return web.ResultReceipt{}, err //nolint:wrapcheck // the web branches on the task service's own errors
	}
	return r.receipt(ctx, out.Task.ID, digest(in))
}

// digest names a publish by its page, its line and the key of the run of the tool that made it, so a call the tool
// repeats is the same version and a deliberate second publish of the same page is a new one.
func digest(in web.ResultInput) string {
	sum := sha256.New()
	for _, part := range []string{in.Key, in.Line, in.Page} {
		sum.Write([]byte(strconv.Itoa(len(part)) + ":" + part))
	}
	return hex.EncodeToString(sum.Sum(nil))
}

// receipt reads the task again, since the answer posted in the same call has moved it on from what the publish left.
func (r results) receipt(ctx context.Context, id domain.TaskID, digest string) (web.ResultReceipt, error) {
	t, err := r.db.Load(ctx, id)
	if err != nil {
		return web.ResultReceipt{}, fmt.Errorf("read task %s: %w", id, err)
	}
	versions, err := r.db.Versions(ctx, id)
	if err != nil {
		return web.ResultReceipt{}, fmt.Errorf("read the versions of %s: %w", id, err)
	}
	for _, v := range versions {
		if v.Digest == digest {
			return web.ResultReceipt{Version: v.N, URL: v.EntryURL, State: t.State.String()}, nil
		}
	}
	return web.ResultReceipt{}, fmt.Errorf("task %s: %w: the version just published is gone", t.ID, domain.ErrNotFound)
}

func (r results) Page(ctx context.Context, id domain.TaskID, version int) ([]byte, error) {
	return r.db.Page(ctx, id, version) //nolint:wrapcheck // the web branches on domain.ErrNotFound
}
