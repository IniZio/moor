package web

import (
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// readEvent reads the event of a task with its source's adapter, for the words that a bare task row lacks: the
// pull request's title, its number and who opened it.
func (s *server) readEvent(t domain.Task) (domain.EventText, bool) {
	if t.Event == nil || s.deps.Events == nil {
		return domain.EventText{}, false
	}
	return s.deps.Events.Read(*t.Event)
}

// eventHow words how an event started a task, as the second line has it after the task number: who opened the
// pull request for a webhook, and who ran the spec for a Run.
func eventHow(t domain.Task, text domain.EventText) string {
	switch {
	case strings.HasPrefix(t.StartedBy, domain.EventStarterPrefix) && text.Verb != "" && text.Actor != "":
		return text.Subject + " " + text.Verb + " by " + text.Actor
	case strings.HasPrefix(t.StartedBy, domain.EventStarterPrefix):
		return text.Subject
	}
	return text.Subject + ", Run by " + t.StartedBy
}

// rowFor draws a task's row with what its event's source reads of the event.
func (s *server) rowFor(it domain.TaskListing) rowView {
	r := rowOf(it, s.now())
	if text, ok := s.readEvent(it.Task); ok {
		r.Title = orText(text.Title, r.Title)
		r.Tail = "#" + string(it.Task.ID) + ": " + eventHow(it.Task, text)
		if it.ReplacedBy != "" {
			r.Tail += ", replaced by #" + string(it.ReplacedBy)
		}
	}
	return r
}

// jsonTaskRead is the task as the API lists it, titled by its event's source when it has one.
func (s *server) jsonTaskRead(it domain.TaskListing) jsonTask {
	j := jsonTaskOf(it)
	if text, ok := s.readEvent(it.Task); ok {
		j.Title = orText(text.Title, j.Title)
	}
	return j
}

// run draws the Run page of a task, with the pull request words where the task has an event.
func (s *server) run(t domain.Task, rows []domain.EventRow) runView {
	v := buildRun(t, rows, s.now())
	text, ok := s.readEvent(t)
	if !ok {
		return v
	}
	v.Row = s.rowFor(domain.TaskListing{Task: t})
	v.Subject = text.Subject
	v.Facts.Detail, v.Facts.DetailURL = text.Subject, safeURL(text.URL)
	if strings.HasPrefix(t.StartedBy, domain.EventStarterPrefix) && text.Verb != "" && text.Actor != "" {
		v.Facts.Detail = text.Actor + " " + text.Verb + " " + text.Subject
	}
	return v
}

func orText(text, fallback string) string {
	if text == "" {
		return fallback
	}
	return text
}
