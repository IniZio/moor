package github

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"

	"github.com/google/go-github/v88/github"

	"github.com/IniZio/moor/internal/domain"
)

// maxPayload is GitHub's own limit on a webhook payload.
const maxPayload = 25 << 20

// Sink takes the deliveries the hook has checked and read.
type Sink interface {
	Receive(ctx context.Context, in domain.Inbound) (domain.InboundResult, error)
}

// Hook serves POST /hooks/github. A delivery whose signature does not match the webhook secret is refused with
// 401. Any event but a pull request is accepted and set aside. The answer is 202, and a failure to store the
// delivery is 500 so that GitHub can send it again.
// With no webhook secret every delivery is refused with 503, since none could be checked.
func (a *Adapter) Hook(sink Sink) http.Handler {
	h := a.hook(sink)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if len(a.secret) == 0 {
			http.Error(w, "the GitHub App's webhook_secret is not set in settings", http.StatusServiceUnavailable)
			return
		}
		h.ServeHTTP(w, r)
	})
}

func (a *Adapter) hook(sink Sink) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Body = http.MaxBytesReader(w, r.Body, maxPayload)
		payload, err := github.ValidatePayload(r, a.secret)
		if err != nil {
			http.Error(w, "the delivery's signature does not match the webhook secret", http.StatusUnauthorized)
			return
		}
		in := domain.Inbound{Source: Source, DeliveryID: github.DeliveryID(r), Event: github.WebHookType(r)}
		if in.DeliveryID == "" {
			http.Error(w, "the delivery has no X-GitHub-Delivery id", http.StatusBadRequest)
			return
		}
		if in.Event != "pull_request" {
			accepted(w, domain.InboundResult{Outcome: "ignored: " + in.Event + " is not read"})
			return
		}
		if err := a.readHook(r.Context(), payload, &in); err != nil {
			log.Printf("github hook %s: %v", in.DeliveryID, err)
			http.Error(w, "the delivery could not be read, send it again", http.StatusInternalServerError)
			return
		}
		res, err := sink.Receive(r.Context(), in)
		if err != nil {
			log.Printf("github hook %s: %v", in.DeliveryID, err)
			http.Error(w, "the delivery could not be stored, send it again", http.StatusInternalServerError)
			return
		}
		accepted(w, res)
	})
}

func accepted(w http.ResponseWriter, res domain.InboundResult) {
	tasks := make([]string, len(res.Tasks))
	for i, id := range res.Tasks {
		tasks[i] = string(id)
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusAccepted)
	if err := json.NewEncoder(w).Encode(map[string]any{"outcome": res.Outcome, "duplicate": res.Duplicate, "tasks": tasks}); err != nil {
		log.Printf("github hook: write the answer: %v", err)
	}
}

// readHook reads a pull_request payload into in. A draft pull request, or one from a fork whose author cannot
// write to the repository, is set aside, and in.Ignored says why.
func (a *Adapter) readHook(ctx context.Context, payload []byte, in *domain.Inbound) error {
	parsed, err := github.ParseWebHook(in.Event, payload)
	if err != nil {
		return fmt.Errorf("parse the payload: %w", err)
	}
	e, ok := parsed.(*github.PullRequestEvent)
	if !ok || e.GetPullRequest() == nil || e.GetRepo() == nil {
		return fmt.Errorf("a pull_request delivery with no pull request or repository")
	}
	in.Action = e.GetAction()
	pr := e.GetPullRequest()
	if pr.GetDraft() {
		in.Ignored = "ignored: draft pull request"
		return nil
	}
	if ok, err := a.mayStart(ctx, e); err != nil || !ok {
		in.Ignored = "ignored: a fork pull request whose author cannot write to the repository"
		return err
	}
	occ, err := prEventOf(e.GetAction(), e.GetRepo(), pr, e.GetSender().GetLogin()).occurrence(domain.EventPullRequest, e.GetAction(), e.GetSender().GetLogin())
	if err != nil {
		return err
	}
	in.Occurrence = &occ
	return nil
}

// mayStart is true for a pull request from the repository itself, and for one from a fork only when its
// author can write to the repository.
func (a *Adapter) mayStart(ctx context.Context, e *github.PullRequestEvent) (bool, error) {
	pr, base := e.GetPullRequest(), e.GetRepo()
	if pr.GetHead().GetRepo().GetFullName() == base.GetFullName() {
		return true, nil
	}
	c, err := a.forRepo(ctx, base.GetOwner().GetLogin(), base.GetName())
	if err != nil {
		return false, err
	}
	level, _, err := c.Repositories.GetPermissionLevel(ctx, base.GetOwner().GetLogin(), base.GetName(), pr.GetUser().GetLogin())
	if err != nil {
		return false, fmt.Errorf("permission of %s on %s: %w", pr.GetUser().GetLogin(), base.GetFullName(), err)
	}
	switch level.GetPermission() {
	case "admin", "write", "maintain":
		return true, nil
	}
	return false, nil
}
