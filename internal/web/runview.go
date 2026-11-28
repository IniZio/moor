package web

import (
	"net/url"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/IniZio/moor/internal/domain"
)

// factsView is the started-by card: who started the task and how, its state, its length and what it checked out.
type factsView struct {
	Started string
	Detail  string
	// DetailURL links Detail, which only an event's source gives.
	DetailURL string
	State     string
	Dur       string
	Ref       string
	Commit    string
}

type failView struct {
	Part   string
	Key    string
	Reason string
}

type resultView struct {
	Name string
	Line string
	At   string
	URL  string
}

type versionView struct {
	Name string
	URL  string
	At   string
	Note string
}

type answerView struct {
	Text string
	At   string
}

// actionsView says which buttons the header shows.
type actionsView struct {
	Cancel bool
	End    bool
	Rerun  bool
}

// runView is everything the Run page draws, built from the task row and its event rows alone.
type runView struct {
	Page    page
	Row     rowView
	Actions actionsView
	Active  string
	Parts   []partView
	Facts   factsView
	Fail    *failView
	Result  *resultView
	Waiting bool
	Earlier []versionView
	Answer  *answerView
	HasSpec bool
	SpecRef string
	LastSeq int64
	// Subject names what an event is about, such as "pull request #90", and is empty without an event.
	Subject string
}

func repoLine(v runView) string {
	if v.Subject == "" {
		return v.Row.Repo
	}
	return v.Row.Repo + ", " + v.Subject
}

// part finds the part a page shows, if the task has reached it.
func (v runView) part(key string) (partView, bool) {
	for _, p := range v.Parts {
		if p.Key == key {
			return p, true
		}
	}
	return partView{}, false
}

func (v runView) has(key string) bool {
	_, ok := v.part(key)
	return ok
}

func (v runView) partOf(key string) partView {
	p, _ := v.part(key)
	return p
}

// buildRun draws the Run page of a task from its row and the event rows in seq order. now is the instant that
// a part still running counts up to.
func buildRun(t domain.Task, rows []domain.EventRow, now time.Time) runView {
	tl := readTimeline(t, rows)
	v := runView{
		Row:     rowOf(domain.TaskListing{Task: t}, now),
		Actions: actionsOf(t),
		Parts:   partsOf(t, tl, now),
		Facts:   factsOf(t, tl, now),
		HasSpec: t.Spec != nil,
	}
	if t.Spec != nil {
		v.SpecRef = t.Spec.File
	}
	if n := len(rows); n > 0 {
		v.LastSeq = rows[n-1].Seq
	}
	v.Fail = failOf(t, tl)
	v.Result, v.Earlier = resultOf(t, tl)
	v.Waiting = v.Result == nil && t.State.Live() && t.State != domain.Waiting
	v.Answer = answerOf(tl)
	return v
}

func actionsOf(t domain.Task) actionsView {
	return actionsView{
		Cancel: t.State.Live(),
		End:    t.Event == nil && t.State == domain.Running,
		Rerun:  t.Event == nil && t.State == domain.Failed,
	}
}

func factsOf(t domain.Task, tl *timeline, now time.Time) factsView {
	f := factsView{State: stateName(t.State), Dur: lasted(t.CreatedAt, endOf(t, now)), Started: startedLabel(t, now), Detail: startedDetail(t)}
	if n, ok := tl.notes[domain.PhaseCheckout]; ok {
		if ref, commit, found := strings.Cut(n.detail, " at "); found {
			f.Ref, f.Commit = ref, commit
		} else {
			f.Ref = n.detail
		}
	} else if t.Commit != "" {
		f.Commit = string(t.Commit)
	}
	return f
}

func stateName(s domain.State) string {
	name := []rune(s.String())
	name[0] = unicode.ToUpper(name[0])
	return string(name)
}

// startedLabel is the first fact's head, such as "Started by Mia 4m ago". A task an event started says which kind.
func startedLabel(t domain.Task, now time.Time) string {
	by := t.StartedBy
	if rest, ok := strings.CutPrefix(t.StartedBy, domain.EventStarterPrefix); ok {
		by = "a " + rest + " event"
		if rest == "schedule" {
			by = "a schedule"
		}
	}
	return "Started by " + by + " " + ago(now, t.CreatedAt)
}

func startedDetail(t domain.Task) string {
	switch {
	case t.Spec == nil:
		return "from a prompt"
	case t.Event != nil && t.Event.Source == "schedule":
		return string(t.Spec.Name)
	case strings.HasPrefix(t.StartedBy, domain.EventStarterPrefix):
		return string(t.Spec.Name) + ", " + string(t.Event.Group)
	}
	return "Run of " + string(t.Spec.Name)
}

func failOf(t domain.Task, tl *timeline) *failView {
	if t.State != domain.Failed {
		return nil
	}
	key := tl.endedIn
	if key == "" {
		key = partAgent
	}
	return &failView{Part: partNames[key], Key: key, Reason: t.Failure}
}

// resultOf reads the newest version as the result, and the others as the earlier versions, newest first.
func resultOf(t domain.Task, tl *timeline) (*resultView, []versionView) {
	n := len(tl.published)
	if n == 0 {
		return nil, nil
	}
	last := tl.published[n-1]
	res := &resultView{Name: "v" + strconv.Itoa(last.Version), Line: last.Line, At: hhmm(last.at), URL: safeURL(last.EntryURL)}
	var earlier []versionView
	for i := n - 2; i >= 0; i-- {
		p := tl.published[i]
		ev := versionView{Name: "v" + strconv.Itoa(p.Version), URL: safeURL(p.EntryURL), At: hhmm(p.at)}
		if tl.reviewBefore(t, p) {
			ev.Note = "before the review"
		}
		earlier = append(earlier, ev)
	}
	return res, earlier
}

// safeURL keeps a result page's address when it is a web address or a path on this site, and gives nothing for
// any other, since an agent chose it and a page must not link a script or a local file.
func safeURL(raw string) string {
	u, err := url.Parse(raw)
	if err != nil {
		return ""
	}
	switch {
	case (u.Scheme == "http" || u.Scheme == "https") && u.Host != "":
		return raw
	case u.Scheme == "" && u.Host == "" && strings.HasPrefix(raw, "/") && !strings.HasPrefix(raw, "//"):
		return raw
	}
	return ""
}

func answerOf(tl *timeline) *answerView {
	n := len(tl.delivered)
	if n == 0 {
		return nil
	}
	d := tl.delivered[n-1]
	verb := "posted"
	if n > 1 {
		verb = "edited"
	}
	return &answerView{Text: "Commented on " + d.Target, At: verb + " " + hhmm(d.at)}
}
