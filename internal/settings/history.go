package settings

import (
	"context"
	"fmt"
	"strings"

	"github.com/aymanbagabas/go-udiff"

	"github.com/IniZio/moor/internal/domain"
)

// History lists every version, newest first.
func (s *Service) History(ctx context.Context) ([]domain.SettingsEntry, error) {
	versions, err := s.store.SettingsVersions(ctx)
	if err != nil {
		return nil, fmt.Errorf("list settings versions: %w", err)
	}
	out := make([]domain.SettingsEntry, len(versions))
	for i, v := range versions {
		before := ""
		if i+1 < len(versions) {
			before = versions[i+1].Text
		}
		out[i] = domain.SettingsEntry{SettingsVersion: v, Diff: Diff(before, v.Text)}
	}
	return out, nil
}

// Diff shows the whole of after, each line marked as added or kept, with the lines of before that went missing.
func Diff(before, after string) []domain.DiffLine {
	edits := udiff.Lines(before, after)
	budget := strings.Count(before, "\n") + strings.Count(after, "\n") + 2
	u, err := udiff.ToUnifiedDiff("", "", before, edits, budget)
	if err != nil || len(u.Hunks) == 0 {
		return kept(after)
	}
	var out []domain.DiffLine
	for _, h := range u.Hunks {
		for _, l := range h.Lines {
			out = append(out, domain.DiffLine{Kind: kind(l.Kind), Text: strings.TrimSuffix(l.Content, "\n")})
		}
	}
	return out
}

func kept(text string) []domain.DiffLine {
	if text == "" {
		return nil
	}
	var out []domain.DiffLine
	for line := range strings.SplitSeq(strings.TrimSuffix(text, "\n"), "\n") {
		out = append(out, domain.DiffLine{Kind: domain.DiffSame, Text: line})
	}
	return out
}

func kind(k udiff.OpKind) domain.DiffKind {
	switch k {
	case udiff.Insert:
		return domain.DiffAdded
	case udiff.Delete:
		return domain.DiffRemoved
	case udiff.Equal:
		return domain.DiffSame
	}
	return domain.DiffSame
}
