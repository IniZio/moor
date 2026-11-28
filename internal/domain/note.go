package domain

import "strings"

// The phases a note row names. A note's text is its phase, or its phase, ": " and a detail, such as
// "checked out: main at 5f0e2b1". The Run page reads the steps of Set up and Review from them.
const (
	PhaseDesk     = "desk up"
	PhaseBox      = "box up"
	PhaseSettings = "settings applied"
	PhaseCheckout = "checked out"
	PhasePrompt   = "prompt sent"
	PhaseReview   = "review sent"
	// PhaseReleased says the keep window of a finished task ended and its containers were released.
	PhaseReleased = "containers released"
)

// NoteText joins a phase and its detail into the text of a note.
func NoteText(phase, detail string) string {
	if detail == "" {
		return phase
	}
	return phase + ": " + detail
}

// NotePhase splits the text of a note into its phase and detail. A text with no detail has an empty one.
func NotePhase(text string) (phase, detail string) {
	phase, detail, _ = strings.Cut(text, ": ")
	return phase, detail
}
