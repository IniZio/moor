package tasktest

import (
	"bytes"
	"context"
	"fmt"
	"maps"
	"slices"
	"sync"

	"github.com/IniZio/moor/internal/domain"
)

// Memory is a Store that enforces what a database would: the version check, the unique task id and
// event key, contiguous event seqs and unique digests.
type Memory struct {
	mu       sync.Mutex
	tasks    map[domain.TaskID]domain.Task
	events   map[domain.TaskID][]domain.EventRow
	versions map[domain.TaskID][]domain.ResultVersion
}

func NewMemory() *Memory {
	return &Memory{
		tasks:    make(map[domain.TaskID]domain.Task),
		events:   make(map[domain.TaskID][]domain.EventRow),
		versions: make(map[domain.TaskID][]domain.ResultVersion),
	}
}

func (m *Memory) Load(_ context.Context, id domain.TaskID) (domain.Task, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	t, ok := m.tasks[id]
	if !ok {
		return domain.Task{}, fmt.Errorf("task %s: %w", id, domain.ErrNotFound)
	}
	return clone(t), nil
}

func (m *Memory) ByKey(_ context.Context, key domain.TaskKey) (domain.Task, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	for _, t := range m.tasks {
		if k, ok := t.Key(); ok && k == key {
			return clone(t), nil
		}
	}
	return domain.Task{}, fmt.Errorf("key %v: %w", key, domain.ErrNotFound)
}

func (m *Memory) LiveInGroup(_ context.Context, project domain.ProjectName, spec domain.SpecName, source domain.EventSource, group domain.EventGroup) ([]domain.Task, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	var live []domain.Task
	for _, t := range m.tasks {
		if k, ok := t.Key(); ok && k.Project == project && k.Spec == spec && k.Source == source && k.Group == group && t.State.Live() {
			live = append(live, clone(t))
		}
	}
	slices.SortFunc(live, func(a, b domain.Task) int { return a.CreatedAt.Compare(b.CreatedAt) })
	return live, nil
}

func (m *Memory) Versions(_ context.Context, id domain.TaskID) ([]domain.ResultVersion, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	return slices.Clone(m.versions[id]), nil
}

func (m *Memory) Save(_ context.Context, w domain.TaskWrite) (domain.Task, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if err := m.check(w); err != nil {
		return domain.Task{}, err
	}
	t := clone(w.Task)
	t.Version = w.Task.Version + 1
	m.tasks[t.ID] = t
	m.events[t.ID] = append(m.events[t.ID], w.Events...)
	if w.Result != nil {
		// A row always belongs to the task being saved, as in the database.
		v := *w.Result
		v.Task = t.ID
		m.versions[t.ID] = append(m.versions[t.ID], v)
	}
	return clone(t), nil
}

// Events returns every row of a task, for tests to inspect.
func (m *Memory) Events(id domain.TaskID) []domain.EventRow {
	m.mu.Lock()
	defer m.mu.Unlock()
	return slices.Clone(m.events[id])
}

// Tasks returns every stored task.
func (m *Memory) Tasks() []domain.Task {
	m.mu.Lock()
	defer m.mu.Unlock()
	all := make([]domain.Task, 0, len(m.tasks))
	for _, t := range m.tasks {
		all = append(all, clone(t))
	}
	slices.SortFunc(all, func(a, b domain.Task) int { return compareID(a.ID, b.ID) })
	return all
}

func compareID(a, b domain.TaskID) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	}
	return 0
}

func (m *Memory) check(w domain.TaskWrite) error {
	cur, exists := m.tasks[w.Task.ID]
	if err := m.checkRow(w, cur, exists); err != nil {
		return err
	}
	if err := checkEvents(w, cur.LastSeq); err != nil {
		return err
	}
	if w.Result == nil {
		return nil
	}
	have := m.versions[w.Task.ID]
	if w.Result.N != len(have)+1 || slices.ContainsFunc(have, func(v domain.ResultVersion) bool { return v.Digest == w.Result.Digest }) {
		return fmt.Errorf("task %s result %d: %w", w.Task.ID, w.Result.N, domain.ErrExists)
	}
	return nil
}

func (m *Memory) checkRow(w domain.TaskWrite, cur domain.Task, exists bool) error {
	switch {
	case w.Create && exists:
		return fmt.Errorf("task %s: %w", w.Task.ID, domain.ErrExists)
	case w.Create:
		return m.checkKey(w.Task)
	case !exists:
		return fmt.Errorf("task %s: %w", w.Task.ID, domain.ErrNotFound)
	case cur.Version != w.Task.Version:
		return fmt.Errorf("task %s at version %d, write has %d: %w", w.Task.ID, cur.Version, w.Task.Version, domain.ErrStaleVersion)
	}
	return nil
}

func (m *Memory) checkKey(t domain.Task) error {
	key, ok := t.Key()
	if !ok {
		return nil
	}
	for _, other := range m.tasks {
		if k, ok := other.Key(); ok && k == key {
			return fmt.Errorf("key %v: %w", key, domain.ErrExists)
		}
	}
	return nil
}

func checkEvents(w domain.TaskWrite, lastSeq int64) error {
	seq := lastSeq
	for _, ev := range w.Events {
		seq++
		if ev.Seq != seq || ev.Task != w.Task.ID {
			return fmt.Errorf("task %s event seq %d, want %d", w.Task.ID, ev.Seq, seq)
		}
	}
	if w.Task.LastSeq != seq {
		return fmt.Errorf("task %s last_seq %d, events end at %d", w.Task.ID, w.Task.LastSeq, seq)
	}
	return nil
}

func clone(t domain.Task) domain.Task {
	t.Pins = maps.Clone(t.Pins)
	t.Refs = maps.Clone(t.Refs)
	if t.Spec != nil {
		sp := *t.Spec
		t.Spec = &sp
	}
	if t.Event != nil {
		ev := *t.Event
		ev.Context = bytes.Clone(ev.Context)
		t.Event = &ev
	}
	return t
}
