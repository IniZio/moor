package task

import (
	"context"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
)

// AnswerFailed records that the answer for a version was not posted, and why. The task stays delivering. The
// publish that made the version, made again with the same digest, reports the delivery it still owes, and that
// is how a failed answer is tried again.
func (s *Service) AnswerFailed(ctx context.Context, id domain.TaskID, version int, reason string) (Outcome, error) {
	return s.simple(ctx, id, func(_ env, t domain.Task) (decision, error) {
		if t.State != domain.Delivering {
			return decision{}, fmt.Errorf("task %s: %w: an answer is only posted while the task is delivering", id, domain.ErrRefused)
		}
		text := fmt.Sprintf("the answer for version %d was not posted: %s", version, reason)
		return decision{task: t, rows: []row{{domain.KindError, domain.ErrorData{Text: text}}}}, nil
	})
}
