package domain

import "errors"

var (
	ErrNotFound     = errors.New("not found")
	ErrStaleVersion = errors.New("stale task version")
	// ErrExists is a create that hit the task id or the trigger key of another task.
	ErrExists = errors.New("already exists")
)

// TaskWrite is one atomic write of a task, its new events and at most one new result version.
//
// Task.Version is the version the writer loaded, zero for a create. The store refuses a stale
// one with ErrStaleVersion and keeps Version+1. Task.LastSeq is the seq of the last event.
type TaskWrite struct {
	Task   Task
	Create bool
	Events []EventRow
	Result *ResultVersion
}
