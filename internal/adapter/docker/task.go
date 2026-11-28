package docker

import (
	"context"
	"fmt"
	"slices"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// Up makes the host match s and waits until every part answers. It reads what exists and changes
// only what differs: a missing or stopped part is created again, and a Box of another key is
// replaced with its volumes, keeping the desk and its volume. A key older than the host's is refused.
func (d *Docker) Up(ctx context.Context, s Spec) error {
	if err := checkSpec(s); err != nil {
		return err
	}
	have, err := d.taskContainers(ctx, s.Key.Task)
	if err != nil {
		return err
	}
	if cur, ok := newest(have); ok && cur.Newer(s.Key) {
		return fmt.Errorf("%w: up %+v, the host has %+v", ErrStale, s.Key, cur)
	}
	l, err := d.ensureNetworks(ctx, s)
	if err != nil {
		return err
	}
	if err := d.converge(ctx, l, staleParts(s.Key, have)); err != nil {
		return err
	}
	return d.waitReady(ctx, l)
}

func (d *Docker) converge(ctx context.Context, l layout, stale []Part) error {
	if len(stale) == 0 {
		return nil
	}
	if err := d.ensureVolumes(ctx, l.spec.Key, taskVolumes); err != nil {
		return err
	}
	if err := d.initVolumes(ctx, l); err != nil {
		return err
	}
	for _, p := range stale {
		if err := d.replace(ctx, l, p); err != nil {
			return err
		}
	}
	return nil
}

func staleParts(k Key, have []Container) []Part {
	var stale []Part
	for _, p := range parts {
		i := slices.IndexFunc(have, func(c Container) bool { return c.Part == p })
		if i < 0 || !have[i].Running || (p == Box && have[i].Key != k) {
			stale = append(stale, p)
		}
	}
	return stale
}

// replace removes the part, and for the Box also its volumes of another generation, before
// creating it again.
func (d *Docker) replace(ctx context.Context, l layout, p Part) error {
	if _, err := d.run(ctx, "rm", "-f", d.name(l.spec.Key.Task, string(p))); err != nil {
		return err
	}
	if p == Box {
		if err := d.dropBoxVolumes(ctx, l.spec.Key); err != nil {
			return err
		}
		if err := d.ensureVolumes(ctx, l.spec.Key, boxVolumes); err != nil {
			return err
		}
	}
	return d.create(ctx, l, p)
}

// Recreate replaces the Box of a task that is up with one of s.Key, a later generation. The desk,
// the proxies and the shared volumes stay.
func (d *Docker) Recreate(ctx context.Context, s Spec) error {
	have, err := d.taskContainers(ctx, s.Key.Task)
	if err != nil {
		return err
	}
	if len(have) == 0 {
		return fmt.Errorf("%w: %s", ErrNotUp, s.Key.Task)
	}
	return d.Up(ctx, s)
}

// Down removes every container, network and volume of the task. Nothing left is not an error.
func (d *Docker) Down(ctx context.Context, k Key) error {
	if err := checkTask(k.Task); err != nil {
		return err
	}
	have, err := d.taskContainers(ctx, k.Task)
	if err != nil {
		return err
	}
	if cur, ok := newest(have); ok && cur.Newer(k) {
		return fmt.Errorf("%w: down %+v, the host has %+v", ErrStale, k, cur)
	}
	names := make([]string, len(have))
	for i, c := range have {
		names[i] = c.Name
	}
	if err := d.remove(ctx, "container", names); err != nil {
		return err
	}
	if err := d.removeTaskObjects(ctx, "network", k.Task); err != nil {
		return err
	}
	return d.removeTaskObjects(ctx, "volume", k.Task)
}

func (d *Docker) removeTaskObjects(ctx context.Context, kind string, t domain.TaskID) error {
	objs, err := d.taskObjects(ctx, kind, t)
	if err != nil {
		return err
	}
	names := make([]string, len(objs))
	for i, o := range objs {
		names[i] = o.Name
	}
	return d.remove(ctx, kind, names)
}

// List finds every task of this prefix on the host by label, so a restarted runner re-adopts them.
func (d *Docker) List(ctx context.Context) ([]Task, error) {
	cs, err := d.containers(ctx)
	if err != nil {
		return nil, err
	}
	byTask := map[domain.TaskID][]Container{}
	for _, c := range cs {
		byTask[c.Key.Task] = append(byTask[c.Key.Task], c)
	}
	tasks := make([]Task, 0, len(byTask))
	for _, group := range byTask {
		slices.SortFunc(group, func(a, b Container) int { return strings.Compare(a.Name, b.Name) })
		k, _ := newest(group)
		tasks = append(tasks, Task{Key: k, Containers: group})
	}
	slices.SortFunc(tasks, func(a, b Task) int { return strings.Compare(string(a.Key.Task), string(b.Key.Task)) })
	return tasks, nil
}
