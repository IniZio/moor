package task

import (
	"context"
	"fmt"
	"slices"

	"github.com/IniZio/moor/internal/domain"
)

// PublishInput is one publish call. Digest names the page's content.
type PublishInput struct {
	Line     string
	EntryURL string
	Digest   string
}

// DeliveredInput says the bridge posted the delivery for Version.
type DeliveredInput struct {
	Target     string
	ExternalID string
	Version    int
}

// Publish records a result version. The same digest again is the same version, and reports the
// effect that version still owes. A trigger recipe with a review turn does not deliver the first one.
func (s *Service) Publish(ctx context.Context, id domain.TaskID, in PublishInput) (Outcome, error) {
	if in.Digest == "" || in.EntryURL == "" {
		return Outcome{}, fmt.Errorf("%w: a publish needs a digest and an entry url", ErrBadInput)
	}
	return s.apply(ctx, id, func(ctx context.Context, e env, t domain.Task) (decision, error) {
		versions, err := s.store.Versions(ctx, id)
		if err != nil {
			return decision{}, fmt.Errorf("load versions of %s: %w", id, err)
		}
		return e.publish(t, versions, in)
	})
}

func (e env) publish(t domain.Task, versions []domain.ResultVersion, in PublishInput) (decision, error) {
	review := e.wantsReview(t) && len(versions) == 0
	if i := slices.IndexFunc(versions, func(v domain.ResultVersion) bool { return v.Digest == in.Digest }); i >= 0 {
		return decision{task: t, effects: e.owed(t, versions[i], len(versions))}, nil
	}
	v := domain.ResultVersion{Task: t.ID, N: len(versions) + 1, EntryURL: in.EntryURL, Line: in.Line, Digest: in.Digest, PublishedAt: e.now}
	published := row{domain.KindPublished, domain.PublishedData{Version: v.N, Line: in.Line, EntryURL: in.EntryURL}}
	if review {
		if _, err := domain.Next(t.Flow(), t.State, domain.Published); err != nil {
			return decision{}, fmt.Errorf("task %s: %w", t.ID, err)
		}
		return decision{task: t, rows: []row{published}, result: &v, effects: e.owed(t, v, v.N)}, nil
	}
	next, rows, err := e.move(t, domain.Published, fmt.Sprintf("published version %d", v.N))
	if err != nil {
		return decision{}, err
	}
	return decision{task: next, rows: append([]row{published}, rows...), result: &v, effects: e.owed(next, v, v.N)}, nil
}

func (e env) wantsReview(t domain.Task) bool {
	if t.Flow() != domain.Triggered || e.recipes == nil {
		return false
	}
	r, ok := e.recipes(t.Kind)
	return ok && r.ReviewTurn
}

// owed derives the effect of version v from the state alone: a delivery while delivering, or the
// review turn while the first version waits for it.
func (e env) owed(t domain.Task, v domain.ResultVersion, latest int) []Effect {
	if v.N != latest {
		return nil
	}
	fx := Effect{Task: t.ID, Runner: t.Runner, Run: t.Run, Gen: t.Gen, Version: v.N}
	switch {
	case t.State == domain.Delivering:
		fx.Kind = Deliver
	case t.State == domain.Running && v.N == 1 && e.wantsReview(t):
		fx.Kind = SendReviewTurn
	default:
		return nil
	}
	return []Effect{fx}
}

// Delivered ends a delivery. A delivery for any version but the newest is refused as stale.
func (s *Service) Delivered(ctx context.Context, id domain.TaskID, in DeliveredInput) (Outcome, error) {
	return s.apply(ctx, id, func(ctx context.Context, e env, t domain.Task) (decision, error) {
		if err := s.checkNewest(ctx, t, in.Version); err != nil {
			return decision{}, err
		}
		d, err := e.run(t, op{event: domain.Delivered, settled: []domain.State{domain.Done}, reason: fmt.Sprintf("delivered version %d", in.Version)})
		if err != nil || len(d.rows) == 0 {
			return d, err
		}
		delivered := row{domain.KindDelivered, domain.DeliveredData{Target: in.Target, ExternalID: in.ExternalID, Version: in.Version}}
		d.rows = append([]row{delivered}, d.rows...)
		return d, nil
	})
}

func (s *Service) checkNewest(ctx context.Context, t domain.Task, version int) error {
	if t.State != domain.Delivering {
		return nil
	}
	versions, err := s.store.Versions(ctx, t.ID)
	if err != nil {
		return fmt.Errorf("load versions of %s: %w", t.ID, err)
	}
	if version != len(versions) {
		return fmt.Errorf("task %s: %w: delivered version %d, newest is %d", t.ID, domain.ErrRefused, version, len(versions))
	}
	return nil
}
