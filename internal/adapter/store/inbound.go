package store

import (
	"context"
	"fmt"

	"github.com/IniZio/moor/internal/domain"
)

// RecordInbound stores a delivery and says whether it was the first of its id. A delivery already stored is left
// as it is.
func (s *Store) RecordInbound(ctx context.Context, ev domain.InboundEvent) (bool, error) {
	tag, err := s.db.Exec(ctx, `INSERT INTO inbound_events (source, delivery_id, event, action, received_at, outcome)
		VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (source, delivery_id) DO NOTHING`,
		string(ev.Source), ev.DeliveryID, ev.Event, ev.Action, ev.ReceivedAt.UTC(), ev.Outcome)
	if err != nil {
		return false, fmt.Errorf("record delivery %s: %w", ev.DeliveryID, err)
	}
	return tag.RowsAffected() == 1, nil
}

// LinkInbound says that a delivery started a task.
func (s *Store) LinkInbound(ctx context.Context, source domain.EventSource, delivery string, id domain.TaskID) error {
	_, err := s.db.Exec(ctx, "INSERT INTO inbound_event_tasks (source, delivery_id, task_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
		string(source), delivery, string(id))
	if err != nil {
		return fmt.Errorf("link delivery %s to task %s: %w", delivery, id, err)
	}
	return nil
}

// FinishInbound sets what the portal did with a delivery.
func (s *Store) FinishInbound(ctx context.Context, source domain.EventSource, delivery, outcome string) error {
	if _, err := s.db.Exec(ctx, "UPDATE inbound_events SET outcome = $3 WHERE source = $1 AND delivery_id = $2", string(source), delivery, outcome); err != nil {
		return fmt.Errorf("finish delivery %s: %w", delivery, err)
	}
	return nil
}

// Inbound reads a stored delivery and the tasks it started, in the order they were made. A delivery never stored
// is domain.ErrNotFound.
func (s *Store) Inbound(ctx context.Context, source domain.EventSource, delivery string) (domain.InboundEvent, []domain.TaskID, error) {
	ev := domain.InboundEvent{Source: source, DeliveryID: delivery}
	err := s.db.QueryRow(ctx, "SELECT event, action, received_at, outcome FROM inbound_events WHERE source = $1 AND delivery_id = $2",
		string(source), delivery).Scan(&ev.Event, &ev.Action, &ev.ReceivedAt, &ev.Outcome)
	if err != nil {
		return ev, nil, notFound(err, fmt.Sprintf("delivery %s", delivery))
	}
	ev.ReceivedAt = ev.ReceivedAt.UTC()
	rows, err := s.db.Query(ctx, `SELECT t.task_id FROM inbound_event_tasks t JOIN tasks k ON k.id = t.task_id
		WHERE t.source = $1 AND t.delivery_id = $2 ORDER BY k.created_at, k.id`, string(source), delivery)
	if err != nil {
		return ev, nil, fmt.Errorf("tasks of delivery %s: %w", delivery, err)
	}
	defer rows.Close()
	var ids []domain.TaskID
	for rows.Next() {
		var id domain.TaskID
		if err := rows.Scan(&id); err != nil {
			return ev, nil, fmt.Errorf("tasks of delivery %s: %w", delivery, err)
		}
		ids = append(ids, id)
	}
	if err := rows.Err(); err != nil {
		return ev, nil, fmt.Errorf("tasks of delivery %s: %w", delivery, err)
	}
	return ev, ids, nil
}
