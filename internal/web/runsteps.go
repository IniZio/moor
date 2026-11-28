package web

import (
	"strconv"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// markPending is the icon of a step the task has not reached.
const markPending domain.StateGroup = "pending"

type stepView struct {
	Name string
	Mark domain.StateGroup
	Dur  string
	Log  []string
}

type partView struct {
	Key   string
	Name  string
	Mark  domain.StateGroup
	Dur   string
	Head  string
	Steps []stepView
}

// head words a part's state and time, such as "Set up done in 31s" or "Agent running for 3m 48s".
func (p partView) head() string {
	switch p.Mark {
	case domain.GroupRunning:
		return "running for " + p.Dur
	case domain.GroupFailed:
		return "failed in " + p.Dur
	case domain.GroupCancelled:
		return "cancelled after " + p.Dur
	case domain.GroupDone, domain.GroupQueued:
	}
	return "done in " + p.Dur
}

// partsOf lists the parts the task has reached, each with its steps.
func partsOf(t domain.Task, tl *timeline, now time.Time) []partView {
	var out []partView
	for _, key := range partKeys {
		if !tl.reached(key) {
			continue
		}
		p := partView{Key: key, Name: partNames[key], Mark: tl.markOf(key), Dur: words(tl.total(key, now))}
		p.Head = p.head()
		switch key {
		case partSetup:
			p.Steps = setupSteps(t, tl, p.Mark, now)
		case partReview:
			p.Steps = reviewSteps(t, tl, p.Mark, now)
		case partAnswer:
			p.Steps = answerSteps(tl, p.Mark, now)
		}
		out = append(out, p)
	}
	return out
}

// marker is where a step ends. A step whose own marker is missing ends where the next step's does, since a step
// that a later one follows has been passed.
type marker struct {
	name  string
	end   time.Time
	lines func() []string
}

// stepsOf draws the steps from their markers. A step is done once its end is known, running while it is the first
// step with no end and the part is running, failed or cancelled at that step when the part ended there, and
// pending after it.
func stepsOf(start time.Time, marks []marker, partMark domain.StateGroup, now time.Time) []stepView {
	out := make([]stepView, len(marks))
	from := start
	reachedOpen := false
	for i, m := range marks {
		end := firstEnd(marks[i:])
		s := stepView{Name: m.name, Mark: markPending}
		switch {
		case !end.IsZero():
			s.Mark, s.Dur = domain.GroupDone, lasted(from, end)
			from = end
		case !reachedOpen:
			reachedOpen = true
			s.Mark, s.Dur = partMark, lasted(from, now)
		}
		if s.Mark != markPending {
			s.Log = m.lines()
		}
		out[i] = s
	}
	return out
}

func firstEnd(marks []marker) time.Time {
	for _, m := range marks {
		if !m.end.IsZero() {
			return m.end
		}
	}
	return time.Time{}
}

func setupSteps(t domain.Task, tl *timeline, partMark domain.StateGroup, now time.Time) []stepView {
	checkout := tl.notes[domain.PhaseCheckout]
	ready := latest(tl.notes[domain.PhaseDesk].at, tl.notes[domain.PhaseBox].at, tl.notes[domain.PhaseSettings].at)
	marks := []marker{
		{"Wait for a machine", tl.picked, func() []string { return waitLines(t, tl) }},
		{"Start desk and Box", ready, func() []string { return readyLines(tl) }},
		{"Check out " + string(t.Repo.Path), checkout.at, func() []string { return checkoutLines(checkout) }},
		{"Send the prompt", orTime(tl.notes[domain.PhasePrompt].at, tl.up), func() []string { return promptLines(t) }},
	}
	return stepsOf(tl.queued, marks, partMark, now)
}

func waitLines(t domain.Task, tl *timeline) []string {
	out := []string{"queued at " + clock(tl.queued)}
	if !tl.picked.IsZero() {
		out = append(out, "picked by "+string(t.Runner)+" at "+clock(tl.picked))
	}
	return out
}

func readyLines(tl *timeline) []string {
	var out []string
	for _, phase := range []string{domain.PhaseDesk, domain.PhaseBox, domain.PhaseSettings} {
		if n, ok := tl.notes[phase]; ok {
			out = append(out, n.text)
		}
	}
	return out
}

// checkoutLines splits a note like "main at 5f0e2b1" into the ref and the commit, as two lines.
func checkoutLines(n note) []string {
	if n.text == "" {
		return nil
	}
	if ref, commit, ok := strings.Cut(n.detail, " at "); ok {
		return []string{ref, "at " + commit}
	}
	return []string{n.detail}
}

func promptLines(t domain.Task) []string {
	from := "typed by " + t.StartedBy
	if t.Spec != nil {
		from = "from " + t.Spec.File
	}
	return append([]string{from, ""}, lines(t.Prompt)...)
}

func reviewSteps(t domain.Task, tl *timeline, partMark domain.StateGroup, now time.Time) []stepView {
	first := tl.published[0]
	var next time.Time
	if len(tl.published) > 1 {
		next = tl.published[1].at
	}
	marks := []marker{
		{"Send the review prompt", tl.notes[domain.PhaseReview].at, func() []string { return reviewLines(t, first) }},
		{"Wait for the next publish", next, func() []string { return nextPublishLines(tl) }},
	}
	return stepsOf(first.at, marks, partMark, now)
}

func reviewLines(t domain.Task, first published) []string {
	from := "from the task spec"
	if t.Spec != nil {
		from = "from " + t.Spec.File
	}
	return append([]string{from + ", after v" + strconv.Itoa(first.Version) + " at " + hhmm(first.at), ""}, lines(t.ReviewPrompt)...)
}

func nextPublishLines(tl *timeline) []string {
	if len(tl.published) < 2 {
		return nil
	}
	p := tl.published[1]
	return []string{"published v" + strconv.Itoa(p.Version) + " at " + hhmm(p.at)}
}

// answerSteps has one step for each answer posted, and a running one while the next is still being posted.
func answerSteps(tl *timeline, partMark domain.StateGroup, now time.Time) []stepView {
	var out []stepView
	for _, d := range tl.delivered {
		out = append(out, stepView{
			Name: "Comment on " + d.Target, Mark: domain.GroupDone, Dur: lasted(tl.answerStart(d.at), d.at),
			Log: []string{d.Target + ", answer for v" + strconv.Itoa(d.Version) + " at " + clock(d.at), "id " + d.ExternalID},
		})
	}
	if partMark != domain.GroupDone || len(out) == 0 {
		out = append(out, stepView{Name: "Post the answer", Mark: partMark, Dur: words(tl.lastStay(partAnswer, now))})
	}
	return out
}

func latest(ts ...time.Time) time.Time {
	var out time.Time
	for _, t := range ts {
		if t.After(out) {
			out = t
		}
	}
	return out
}

func orTime(a, b time.Time) time.Time {
	if a.IsZero() {
		return b
	}
	return a
}
