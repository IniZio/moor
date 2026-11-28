package domain

import (
	"errors"
	"fmt"
	"path"
	"strings"
)

// Entry is one repository of a project, checked out at Dir. Setup is run there, in list order; empty means none.
type Entry struct {
	Repo  Repo
	Ref   string
	Dir   Dir
	Setup string
}

type Project struct {
	name    ProjectName
	entries []Entry
}

var ErrBadProject = errors.New("bad project")

// NewProject is the only way to make a Project, so every Project has entries with distinct, local dirs.
func NewProject(name ProjectName, entries []Entry) (Project, error) {
	if name == "" {
		return Project{}, fmt.Errorf("%w: no name", ErrBadProject)
	}
	if len(entries) == 0 {
		return Project{}, fmt.Errorf("%w: %s has no entries", ErrBadProject, name)
	}
	seen := make(map[Dir]bool, len(entries))
	for _, e := range entries {
		if err := checkEntry(e); err != nil {
			return Project{}, fmt.Errorf("%w: %s: %w", ErrBadProject, name, err)
		}
		if seen[e.Dir] {
			return Project{}, fmt.Errorf("%w: %s: dir %q twice", ErrBadProject, name, e.Dir)
		}
		seen[e.Dir] = true
	}
	return Project{name: name, entries: append([]Entry(nil), entries...)}, nil
}

func checkEntry(e Entry) error {
	if e.Repo.Host == "" || e.Repo.Path == "" || e.Ref == "" {
		return fmt.Errorf("entry %q needs a repo host, a repo path and a ref", e.Dir)
	}
	d := string(e.Dir)
	if d == "" || path.IsAbs(d) || path.Clean(d) != d || d == ".." || strings.HasPrefix(d, "../") {
		return fmt.Errorf("entry dir %q is not a clean relative path", d)
	}
	return nil
}

func (p Project) Name() ProjectName { return p.name }

func (p Project) Entries() []Entry { return append([]Entry(nil), p.entries...) }
