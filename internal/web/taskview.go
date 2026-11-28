package web

import (
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// taskTitle is the first line of a plain prompt, or the name of the spec that started the task.
func taskTitle(t domain.Task) string {
	if t.Spec != nil {
		return string(t.Spec.Name)
	}
	line, _, _ := strings.Cut(strings.TrimSpace(t.Prompt), "\n")
	return strings.TrimSpace(line)
}
