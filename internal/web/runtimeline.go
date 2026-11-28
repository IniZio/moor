package web

import (
	"encoding/json"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// The keys of the parts, which are also the last path segment of a part's page.
const (
	partSetup  = "setup"
	partAgent  = "agent"
	partReview = "review"
	partAnswer = "answer"
)

var partKeys = []string{partSetup, partAgent, partReview, partAnswer}

var partNames = map[string]string{partSetup: "Set up", partAgent: "Agent", partReview: "Review", partAnswer: "Answer"}

type span struct{ from, to time.Time }

// published is a published row read as a result version.
type published struct {
	seq int64
	at  time.Time
	domain.PublishedData
}

type delivered struct {
	at time.Time
	domain.DeliveredData
}

type note struct {
	at     time.Time
	text   string
	detail string
}

// timeline is what the event rows of a task say about its parts: when each part ran, how it ended, and the
// facts that fill its steps. The rows are the only source, so a replica that reads them draws the same page.
type timeline struct {
	spans     map[string][]span
	queued    time.Time
	picked    time.Time
	up        time.Time
	endState  domain.State
	endedIn   string
	notes     map[string]note
	published []published
	delivered []delivered
	// reviewSeq is the seq of the row that sent the review prompt, zero while it is not sent.
	reviewSeq int64
	current   string
	since     time.Time
}

func newTimeline() *timeline {
	return &timeline{spans: map[string][]span{}, notes: map[string]note{}}
}

// readTimeline folds the rows of a task, in seq order, into its timeline. A task with a review prompt hands its
// agent over to review at the first publish.
func readTimeline(t domain.Task, rows []domain.EventRow) *timeline {
	tl := newTimeline()
	for _, r := range rows {
		switch r.Kind {
		case domain.KindState:
			tl.state(r)
		case domain.KindNote:
			tl.note(r)
		case domain.KindPublished:
			tl.publish(t, r)
		case domain.KindDelivered:
			var d domain.DeliveredData
			if json.Unmarshal(r.Data, &d) == nil {
				tl.delivered = append(tl.delivered, delivered{at: r.At, DeliveredData: d})
			}
		case domain.KindBox, domain.KindHome, domain.KindPolicy, domain.KindError:
		}
	}
	return tl
}

func (tl *timeline) open(part string, at time.Time) {
	tl.close(at)
	tl.current, tl.since = part, at
}

func (tl *timeline) close(at time.Time) {
	if tl.current == "" {
		return
	}
	tl.spans[tl.current] = append(tl.spans[tl.current], span{from: tl.since, to: at})
	tl.current = ""
}

func (tl *timeline) state(r domain.EventRow) {
	var d domain.StateData
	if json.Unmarshal(r.Data, &d) != nil {
		return
	}
	to, err := domain.ParseState(d.To)
	if err != nil {
		return
	}
	switch to {
	case domain.Waiting:
		tl.queued = r.At
		tl.open(partSetup, r.At)
	case domain.Starting:
		tl.picked = r.At
		if tl.current != partSetup {
			tl.open(partSetup, r.At)
		}
	case domain.Running:
		if tl.up.IsZero() {
			tl.up = r.At
		}
		tl.open(partAgent, r.At)
	case domain.Delivering:
		tl.open(partAnswer, r.At)
	case domain.Done, domain.Failed, domain.Cancelled, domain.Superseded:
		tl.endState, tl.endedIn = to, tl.current
		tl.close(r.At)
	}
}

func (tl *timeline) note(r domain.EventRow) {
	var d domain.NoteData
	if json.Unmarshal(r.Data, &d) != nil {
		return
	}
	phase, detail := domain.NotePhase(d.Text)
	tl.notes[phase] = note{at: r.At, text: d.Text, detail: detail}
	if phase == domain.PhaseReview && tl.reviewSeq == 0 {
		tl.reviewSeq = r.Seq
	}
}

func (tl *timeline) publish(t domain.Task, r domain.EventRow) {
	p := published{seq: r.Seq, at: r.At}
	if json.Unmarshal(r.Data, &p.PublishedData) != nil {
		return
	}
	tl.published = append(tl.published, p)
	switch {
	case t.ReviewPrompt != "" && len(tl.published) == 1 && tl.current == partAgent:
		tl.open(partReview, r.At)
	case tl.current == partReview:
		tl.close(r.At)
	}
}

// reached tells whether the task has gone into the part, so that the left list shows it.
func (tl *timeline) reached(part string) bool { return len(tl.spans[part]) > 0 || tl.current == part }

// total is how long the task has been in the part, counting a stay still open up to now.
func (tl *timeline) total(part string, now time.Time) time.Duration {
	var sum time.Duration
	for _, s := range tl.spans[part] {
		sum += max(s.to.Sub(s.from), 0)
	}
	if tl.current == part {
		sum += max(now.Sub(tl.since), 0)
	}
	return sum
}

// answerStart is when the stay in Answer that posted at at began.
func (tl *timeline) answerStart(at time.Time) time.Time {
	begin := at
	for _, s := range tl.spans[partAnswer] {
		if !s.from.After(at) {
			begin = s.from
		}
	}
	return begin
}

// lastStay is how long the latest stay in the part lasted, up to now when it is still open.
func (tl *timeline) lastStay(part string, now time.Time) time.Duration {
	if tl.current == part {
		return max(now.Sub(tl.since), 0)
	}
	if n := len(tl.spans[part]); n > 0 {
		return max(tl.spans[part][n-1].to.Sub(tl.spans[part][n-1].from), 0)
	}
	return 0
}

// markOf is the icon a part shows: running while the task is in it, failed or cancelled when the task ended there,
// and done once the task has moved past it.
func (tl *timeline) markOf(part string) domain.StateGroup {
	switch {
	case tl.current == part:
		return domain.GroupRunning
	case tl.endedIn != part:
		return domain.GroupDone
	case tl.endState == domain.Failed:
		return domain.GroupFailed
	case tl.endState == domain.Cancelled || tl.endState == domain.Superseded:
		return domain.GroupCancelled
	}
	return domain.GroupDone
}

// reviewBefore tells whether the version was published before the review prompt went out.
func (tl *timeline) reviewBefore(t domain.Task, p published) bool {
	if tl.reviewSeq != 0 {
		return p.seq < tl.reviewSeq
	}
	return t.ReviewPrompt != "" && len(tl.published) > 0 && p.seq == tl.published[0].seq
}

func clock(t time.Time) string { return t.UTC().Format("15:04:05") }

func hhmm(t time.Time) string { return t.UTC().Format("15:04") }

func lines(text string) []string { return strings.Split(strings.TrimRight(text, "\n"), "\n") }
