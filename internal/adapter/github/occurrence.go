package github

import (
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/google/go-github/v88/github"

	"github.com/IniZio/moor/internal/domain"
)

// prEvent is the part of a pull request event that a prompt template or a screen reads. It has the shape of
// GitHub's own payload for the fields it keeps, so a spec writes event.pull_request.html_url and means GitHub's field.
// A webhook and a Run on a link both make it, which is why a prompt has one form for both.
type prEvent struct {
	Action     string `json:"action,omitempty"`
	Number     int    `json:"number"`
	Repository struct {
		FullName string `json:"full_name"`
		HTMLURL  string `json:"html_url"`
	} `json:"repository"`
	PullRequest struct {
		Number  int    `json:"number"`
		Title   string `json:"title"`
		Body    string `json:"body"`
		HTMLURL string `json:"html_url"`
		State   string `json:"state"`
		Draft   bool   `json:"draft"`
		Head    struct {
			SHA string `json:"sha"`
			Ref string `json:"ref"`
		} `json:"head"`
		Base struct {
			Ref string `json:"ref"`
		} `json:"base"`
		User struct {
			Login string `json:"login"`
		} `json:"user"`
	} `json:"pull_request"`
	Sender struct {
		Login string `json:"login"`
	} `json:"sender"`
}

// prEventOf keeps the fields of a pull request that the product reads.
func prEventOf(action string, repo *github.Repository, pr *github.PullRequest, sender string) prEvent {
	var c prEvent
	c.Action, c.Number, c.Sender.Login = action, pr.GetNumber(), sender
	c.Repository.FullName, c.Repository.HTMLURL = repo.GetFullName(), repo.GetHTMLURL()
	p := &c.PullRequest
	p.Number, p.Title, p.Body, p.HTMLURL = pr.GetNumber(), pr.GetTitle(), pr.GetBody(), pr.GetHTMLURL()
	p.State, p.Draft = pr.GetState(), pr.GetDraft()
	p.Head.SHA, p.Head.Ref = pr.GetHead().GetSHA(), pr.GetHead().GetRef()
	p.Base.Ref, p.User.Login = pr.GetBase().GetRef(), pr.GetUser().GetLogin()
	return c
}

// occurrence reads the event into the domain's. The prompt template and the answer read the same JSON.
func (c prEvent) occurrence(name domain.EventName, action string, actor string) (domain.Occurrence, error) {
	repo, err := domain.ParseRepo(c.Repository.FullName)
	if err != nil {
		return domain.Occurrence{}, fmt.Errorf("the pull request's repository: %w", err)
	}
	if c.PullRequest.Number == 0 || c.PullRequest.Head.SHA == "" {
		return domain.Occurrence{}, errors.New("the pull request has no number or head commit")
	}
	raw, err := json.Marshal(c)
	if err != nil {
		return domain.Occurrence{}, fmt.Errorf("encode the pull request context: %w", err)
	}
	group := domain.EventGroup(strings.ToLower(c.Repository.FullName) + "#" + strconv.Itoa(c.PullRequest.Number))
	return domain.Occurrence{
		Incoming: domain.Incoming{Source: Source, Name: name, Action: action, Repo: repo, Payload: raw},
		Event:    domain.Event{Source: Source, Group: group, Rev: domain.Revision(c.PullRequest.Head.SHA), Context: raw},
		Actor:    actor,
		Ref:      "refs/pull/" + strconv.Itoa(c.PullRequest.Number) + "/head", Commit: domain.Commit(c.PullRequest.Head.SHA),
	}, nil
}

func prEventFrom(ev domain.Event) (prEvent, bool) {
	var c prEvent
	if err := json.Unmarshal(ev.Context, &c); err != nil || c.PullRequest.Number == 0 {
		return c, false
	}
	return c, true
}

// ReadEvent reads a pull request event for the screens: the pull request's title, "pull request #90", who
// opened or updated it, and where it is. A Run has no action, so it has no verb.
func (*Adapter) ReadEvent(ev domain.Event) (domain.EventText, bool) {
	c, ok := prEventFrom(ev)
	if !ok {
		return domain.EventText{}, false
	}
	return domain.EventText{
		Title: c.PullRequest.Title, Subject: "pull request #" + strconv.Itoa(c.PullRequest.Number),
		Verb: verbOf(c.Action), Actor: c.Sender.Login, URL: c.PullRequest.HTMLURL,
	}, true
}

// verbOf words a pull request action for the second line of a row: opened, or updated for a push.
func verbOf(action string) string {
	switch action {
	case "opened", "reopened", "ready_for_review":
		return "opened"
	case "synchronize":
		return "updated"
	case "closed":
		return "closed"
	}
	return ""
}
