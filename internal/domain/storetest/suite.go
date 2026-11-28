// Package storetest holds the cases every task store must pass. The in-memory store and the
// database store run the same ones, so the memory one can stand in for the other in tests.
package storetest

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// Store is the five calls the task logic makes. It mirrors the interface in package task, which
// this package cannot import.
type Store interface {
	Load(ctx context.Context, id domain.TaskID) (domain.Task, error)
	ByKey(ctx context.Context, key domain.TaskKey) (domain.Task, error)
	LiveInGroup(ctx context.Context, project domain.ProjectName, source domain.TriggerSource, group domain.TriggerGroup) ([]domain.Task, error)
	Versions(ctx context.Context, id domain.TaskID) ([]domain.ResultVersion, error)
	Save(ctx context.Context, w domain.TaskWrite) (domain.Task, error)
}

// Subject is one empty store and a way to read back the event rows it holds, which Store cannot.
type Subject struct {
	Store  Store
	Events func(ctx context.Context, id domain.TaskID) ([]domain.EventRow, error)
}

// Run runs every case on a fresh subject. Times in the cases keep to microseconds, the finest a
// database stores.
func Run(t *testing.T, fresh func(t *testing.T) Subject) {
	t.Helper()
	cases := []struct {
		name string
		run  func(*testing.T, Subject)
	}{
		{"round trip", roundTrip},
		{"person-started has no key", personStarted},
		{"person-started review prompt and reply target", personFields},
		{"missing rows", missing},
		{"duplicate id and key", duplicates},
		{"stale version", staleVersion},
		{"version bumps on every write", versionBumps},
		{"event seq must follow the stored one", badEvents},
		{"result versions", resultVersions},
		{"a refused result leaves the write undone", refusedResultUndoesWrite},
		{"live tasks in a group", liveInGroup},
		{"returned values are copies", copies},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) { c.run(t, fresh(t)) })
	}
}
