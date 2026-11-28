package task

import (
	"context"
	"fmt"
	"slices"
	"strconv"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// EntryURLVersion in a publish's EntryURL stands for the number of the version the publish makes, which only the task
// knows.
const EntryURLVersion = "{version}"

// PublishInput is one publish call. Digest names the page's content. Page is the page itself, kept with the
// version when the portal serves it, and empty when the page lives at EntryURL elsewhere.
type PublishInput struct {
	Line     string
	EntryURL string
	Digest   string
	Page     []byte
}

// DeliveredInput says the bridge posted the delivery for Version.
type DeliveredInput struct {
	Target     string
	ExternalID string
	Version    int
}

// Publish records a result version. The same digest again is the same version, and reports the
// effect that version still owes. A task with a review prompt does not accept the first version:
// the result is accepted on the next publish.
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
	review := t.ReviewPrompt != "" && len(versions) == 0
	if i := slices.IndexFunc(versions, func(v domain.ResultVersion) bool { return v.Digest == in.Digest }); i >= 0 {
		return decision{task: t, effects: e.owed(t, versions[i], len(versions))}, nil
	}
	n := len(versions) + 1
	url := strings.ReplaceAll(in.EntryURL, EntryURLVersion, strconv.Itoa(n))
	v := domain.ResultVersion{Task: t.ID, N: n, EntryURL: url, Line: in.Line, Digest: in.Digest, PublishedAt: e.now}
	published := row{domain.KindPublished, domain.PublishedData{Version: v.N, Line: in.Line, EntryURL: url}}
	if review {
		if _, err := domain.Next(e.flow(t), t.State, domain.Published); err != nil {
			return decision{}, fmt.Errorf("task %s: %w", t.ID, err)
		}
		return decision{task: t, rows: []row{published}, result: &v, page: in.Page, effects: e.owed(t, v, v.N)}, nil
	}
	next, rows, err := e.move(t, domain.Published, fmt.Sprintf("published version %d", v.N))
	if err != nil {
		return decision{}, err
	}
	return decision{task: next, rows: append([]row{published}, rows...), result: &v, page: in.Page, effects: e.owed(next, v, v.N)}, nil
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
	case t.State == domain.Running && v.N == 1 && t.ReviewPrompt != "":
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
		d, err := e.run(t, op{signal: domain.Delivered, settled: []domain.State{domain.Done}, reason: fmt.Sprintf("delivered version %d", in.Version)})
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

// ReviewFailed records that the review prompt was claimed and could not be typed, and why. Nobody asks again.
func (s *Service) ReviewFailed(ctx context.Context, id domain.TaskID, reason string) (Outcome, error) {
	return s.simple(ctx, id, func(_ env, t domain.Task) (decision, error) {
		text := "the review prompt was not typed: " + reason
		return decision{task: t, rows: []row{{domain.KindError, domain.ErrorData{Text: text}}}}, nil
	})
}

// ReviewTurn claims the review turn of a task whose first version waits for it. It records the turn as a note and
// reports it as an effect to the one caller that recorded it: the write checks the task's version under a row lock, so
// when two replicas ask at once the second finds the note and is owed nothing. The turn is at most once. A caller that
// claimed it and could not type it must say so with an error row, because nobody asks again.
func (s *Service) ReviewTurn(ctx context.Context, id domain.TaskID) (Outcome, error) {
	return s.apply(ctx, id, func(ctx context.Context, e env, t domain.Task) (decision, error) {
		owed, err := s.reviewOwed(ctx, t)
		if err != nil || !owed {
			return decision{task: t}, err
		}
		if _, err := domain.Next(e.flow(t), t.State, domain.ReviewSent); err != nil {
			return decision{}, fmt.Errorf("task %s: %w", id, err)
		}
		fx := Effect{Kind: SendReviewTurn, Task: t.ID, Runner: t.Runner, Run: t.Run, Gen: t.Gen, Version: 1}
		return decision{task: t, rows: []row{{domain.KindNote, domain.NoteData{Text: domain.PhaseReview}}}, effects: []Effect{fx}}, nil
	})
}

// reviewOwed is true while a running task with a review prompt holds its first version and has not sent the prompt.
func (s *Service) reviewOwed(ctx context.Context, t domain.Task) (bool, error) {
	if t.ReviewPrompt == "" || t.State != domain.Running {
		return false, nil
	}
	versions, err := s.store.Versions(ctx, t.ID)
	if err != nil {
		return false, fmt.Errorf("load versions of %s: %w", t.ID, err)
	}
	if len(versions) != 1 {
		return false, nil
	}
	sent, err := s.store.HasNote(ctx, t.ID, domain.PhaseReview)
	if err != nil {
		return false, fmt.Errorf("look for the review note of %s: %w", t.ID, err)
	}
	return !sent, nil
}
