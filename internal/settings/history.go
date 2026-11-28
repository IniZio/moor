package settings

import (
	"context"
	"fmt"
	"strings"

	"github.com/aymanbagabas/go-udiff"

	"github.com/IniZio/moor/internal/domain"
)

type DiffKind uint8

const (
	Same DiffKind = iota
	Added
	Removed
)

type DiffLine struct {
	Kind DiffKind
	Text string
}

// Entry is a saved version with its diff against the version before it. Version 1 diffs against nothing.
type Entry struct {
	domain.SettingsVersion
	Diff []DiffLine
}

// History lists every version, newest first.
func (s *Service) History(ctx context.Context) ([]Entry, error) {
	versions, err := s.store.SettingsVersions(ctx)
	if err != nil {
		return nil, fmt.Errorf("list settings versions: %w", err)
	}
	out := make([]Entry, len(versions))
	for i, v := range versions {
		before := ""
		if i+1 < len(versions) {
			before = versions[i+1].Text
		}
		out[i] = Entry{SettingsVersion: v, Diff: Diff(before, v.Text)}
	}
	return out, nil
}

// Diff shows the whole of after, each line marked as added or kept, with the lines of before that went missing.
func Diff(before, after string) []DiffLine {
	edits := udiff.Lines(before, after)
	budget := strings.Count(before, "\n") + strings.Count(after, "\n") + 2
	u, err := udiff.ToUnifiedDiff("", "", before, edits, budget)
	if err != nil || len(u.Hunks) == 0 {
		return kept(after)
	}
	var out []DiffLine
	for _, h := range u.Hunks {
		for _, l := range h.Lines {
			out = append(out, DiffLine{Kind: kind(l.Kind), Text: strings.TrimSuffix(l.Content, "\n")})
		}
	}
	return out
}

func kept(text string) []DiffLine {
	if text == "" {
		return nil
	}
	var out []DiffLine
	for line := range strings.SplitSeq(strings.TrimSuffix(text, "\n"), "\n") {
		out = append(out, DiffLine{Kind: Same, Text: line})
	}
	return out
}

func kind(k udiff.OpKind) DiffKind {
	switch k {
	case udiff.Insert:
		return Added
	case udiff.Delete:
		return Removed
	case udiff.Equal:
		return Same
	}
	return Same
}
