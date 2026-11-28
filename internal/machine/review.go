package machine

import (
	"context"
	"errors"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// reviews types the review prompt into the session of each running task on this machine whose first version waits
// for it. Only the replica that holds the machine's link does, and the task service hands the turn to one caller,
// so a second replica, a restart or the next tick finds it taken.
func (h *holding) reviews(ctx context.Context) error {
	on, err := h.c.cfg.Store.TasksOn(ctx, h.id)
	if err != nil {
		return fmt.Errorf("tasks on the machine: %w", err)
	}
	var errs []error
	for _, t := range on {
		if t.State == domain.Running && t.ReviewPrompt != "" {
			errs = append(errs, h.review(ctx, t))
		}
	}
	return errors.Join(errs...)
}

func (h *holding) review(ctx context.Context, t domain.Task) error {
	out, err := h.c.cfg.Tasks.ReviewTurn(ctx, t.ID)
	if err != nil {
		return fmt.Errorf("claim the review turn of %s: %w", t.ID, err)
	}
	for _, fx := range out.Effects {
		if fx.Kind != task.SendReviewTurn {
			continue
		}
		if err := h.c.Type(ctx, t.ID, t.ReviewPrompt); err != nil {
			if _, recErr := h.c.cfg.Tasks.ReviewFailed(ctx, t.ID, err.Error()); recErr != nil {
				err = errors.Join(err, fmt.Errorf("record the failure: %w", recErr))
			}
			return fmt.Errorf("type the review prompt of %s: %w", t.ID, err)
		}
	}
	return nil
}
