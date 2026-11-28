package runner

import (
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// launchNotes are the steps the runner took to start a task, in order: the desk and Box up, the repository checked
// out, the prompt in the session. checkedOut says what was checked out, such as "main at 5f0e2b19c4d7".
func (r *Runner) launchNotes(checkedOut string) []string {
	return []string{
		domain.NoteText(domain.PhaseDesk, r.cfg.Machine), domain.PhaseBox,
		domain.NoteText(domain.PhaseCheckout, checkedOut), domain.PhasePrompt,
	}
}

// note makes the launch f's notes and sends them. A launch that has its notes already, such as one a repeated start
// joins, sends nothing: the portal gets them again from the levels of each watch it opens.
func (r *Runner) note(j *job, f wire.Fence, checkedOut string) {
	r.mu.Lock()
	if j.noted == f {
		r.mu.Unlock()
		return
	}
	j.noted, j.notes = f, nil
	for _, text := range r.launchNotes(checkedOut) {
		j.notes = append(j.notes, wire.TaskWatchLine{Fence: f, Part: wire.NotePart, Text: text})
	}
	notes := append([]wire.TaskWatchLine(nil), j.notes...)
	r.mu.Unlock()
	for _, l := range notes {
		r.hub.emit(l)
	}
}
