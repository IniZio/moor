package runner

import (
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/wire"
)

// launchNotes are the steps the runner took to start a task, in order: the desk and Box up, the prompt in the
// session. It checks nothing out yet, so it says nothing of a checkout.
func (r *Runner) launchNotes() []string {
	return []string{domain.NoteText(domain.PhaseDesk, r.cfg.Machine), domain.PhaseBox, domain.PhasePrompt}
}

// note makes the launch f's notes and sends them. A launch that has its notes already, such as one a repeated start
// joins, sends nothing: the portal gets them again from the levels of each watch it opens.
func (r *Runner) note(j *job, f wire.Fence) {
	r.mu.Lock()
	if j.noted == f {
		r.mu.Unlock()
		return
	}
	j.noted, j.notes = f, nil
	for _, text := range r.launchNotes() {
		j.notes = append(j.notes, wire.TaskWatchLine{Fence: f, Part: wire.NotePart, Text: text})
	}
	notes := append([]wire.TaskWatchLine(nil), j.notes...)
	r.mu.Unlock()
	for _, l := range notes {
		r.hub.emit(l)
	}
}
