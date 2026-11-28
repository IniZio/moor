package web

import (
	"fmt"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// rowView is a task as one row of the list shows it. Tail is what follows the spec's name on the second line.
type rowView struct {
	ID       string
	Href     string
	Group    domain.StateGroup
	Title    string
	Spec     string
	Tail     string
	Repo     string
	Age      string
	Duration string
}

// taskTitle is the first line of a plain prompt, or the name of the spec that started the task.
func taskTitle(t domain.Task) string {
	if t.Spec != nil {
		return string(t.Spec.Name)
	}
	line, _, _ := strings.Cut(strings.TrimSpace(t.Prompt), "\n")
	return strings.TrimSpace(line)
}

// startedText says how a task started, as the second line has it after the task number.
func startedText(it domain.TaskListing) string {
	t := it.Task
	var how string
	switch {
	case t.Spec == nil:
		how = "started by " + t.StartedBy
	case strings.HasPrefix(t.StartedBy, domain.EventStarterPrefix):
		how = strings.TrimPrefix(t.StartedBy, domain.EventStarterPrefix) + " event"
	default:
		how = "Run by " + t.StartedBy
	}
	if it.ReplacedBy != "" {
		how += ", replaced by #" + string(it.ReplacedBy)
	}
	return how
}

func rowOf(it domain.TaskListing, now time.Time) rowView {
	t := it.Task
	r := rowView{
		ID: string(t.ID), Href: "/tasks/" + string(t.ID), Group: t.State.Group(), Title: taskTitle(t),
		Tail: "#" + string(t.ID) + ": " + startedText(it), Repo: string(t.Repo.Path),
		Age: ago(now, t.CreatedAt), Duration: lasted(t.CreatedAt, endOf(t, now)),
	}
	if t.Spec != nil {
		r.Spec = string(t.Spec.Name)
	}
	return r
}

func endOf(t domain.Task, now time.Time) time.Time {
	if t.EndedAt.IsZero() {
		return now
	}
	return t.EndedAt
}

// ago words the time since t as 12m ago, 2h ago or 3d ago.
func ago(now, t time.Time) string {
	d := max(now.Sub(t), 0)
	switch {
	case d < time.Minute:
		return "just now"
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	}
	return fmt.Sprintf("%dd ago", int(d.Hours()/24))
}

// lasted words the time between two instants as 40s, 9m 41s, 1h 05m or 2d 03h.
func lasted(from, to time.Time) string {
	d := max(to.Sub(from), 0).Round(time.Second)
	secs := int(d.Seconds())
	switch {
	case secs < 60:
		return fmt.Sprintf("%ds", secs)
	case secs < 3600:
		return fmt.Sprintf("%dm %02ds", secs/60, secs%60)
	case secs < 86400:
		return fmt.Sprintf("%dh %02dm", secs/3600, secs%3600/60)
	}
	return fmt.Sprintf("%dd %02dh", secs/86400, secs%86400/3600)
}

func countText(n int, more bool) string {
	switch {
	case more:
		return fmt.Sprintf("Newest %d tasks", n)
	case n == 1:
		return "1 task"
	}
	return fmt.Sprintf("%d tasks", n)
}
