package web

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"strconv"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
)

// BlockedRequests is what the Run page reads and does with a task's blocked requests.
type BlockedRequests interface {
	BlockedRequests(ctx context.Context, task domain.TaskID) ([]domain.BlockedRequest, error)
	Allow(ctx context.Context, task domain.TaskID, id domain.BlockedID, by string) (domain.AllowedLine, error)
	Dismiss(ctx context.Context, task domain.TaskID, id domain.BlockedID, by string) error
}

type blockedRowView struct {
	ID       string
	Request  string
	Times    string
	State    string
	CanAllow bool
}

type blockedView struct {
	Title string
	Rows  []blockedRowView
}

// blockedOf titles the list by its open count, as in "3 blocked requests".
func blockedOf(list []domain.BlockedRequest) blockedView {
	var v blockedView
	open := 0
	for _, b := range list {
		row := blockedRowView{ID: strconv.FormatInt(int64(b.ID), 10), Request: b.Request(), Times: strconv.Itoa(b.Count)}
		switch {
		case b.Decision == domain.Allowed:
			row.State = "Allowed"
		case b.Decision == domain.Dismissed:
			row.State = "Dismissed"
		case !b.Allowable():
			row.State = "Needs a connection"
			open++
		default:
			row.CanAllow = true
			open++
		}
		v.Rows = append(v.Rows, row)
	}
	switch open {
	case 0:
		v.Title = "Blocked requests, all allowed"
	case 1:
		v.Title = "1 blocked request"
	default:
		v.Title = strconv.Itoa(open) + " blocked requests"
	}
	return v
}

// blockedList reads the task's blocked requests for a page. A failed read draws the page without them.
func (s *server) blockedList(ctx context.Context, id domain.TaskID) []domain.BlockedRequest {
	if s.deps.Blocked == nil {
		return nil
	}
	list, err := s.deps.Blocked.BlockedRequests(ctx, id)
	if err != nil {
		log.Printf("blocked requests of %s: %v", id, err)
	}
	return list
}

type jsonBlocked struct {
	ID       int64     `json:"id"`
	Proxy    string    `json:"proxy"`
	Request  string    `json:"request"`
	Reason   string    `json:"reason"`
	Count    int       `json:"count"`
	LastAt   time.Time `json:"last_at"`
	Decision string    `json:"decision"`
}

func jsonBlockedOf(list []domain.BlockedRequest) []jsonBlocked {
	out := make([]jsonBlocked, len(list))
	for i, b := range list {
		out[i] = jsonBlocked{ID: int64(b.ID), Proxy: string(b.Proxy), Request: b.Request(), Reason: string(b.Reason), Count: b.Count, LastAt: b.LastAt, Decision: string(b.Decision)}
	}
	return out
}

// decideBlocked serves POST /api/tasks/{id}/blocked/{bid} with action allow or dismiss, as JSON or a form.
func (s *server) decideBlocked(w http.ResponseWriter, r *http.Request) {
	bid, err := strconv.ParseInt(chi.URLParam(r, "bid"), 10, 64)
	if err != nil || s.deps.Blocked == nil {
		http.NotFound(w, r)
		return
	}
	id, by := taskID(r), personOf(r.Context())
	switch actionOf(w, r) {
	case "allow":
		got, err := s.deps.Blocked.Allow(r.Context(), id, domain.BlockedID(bid), by)
		if err != nil {
			blockedError(w, r, err)
			return
		}
		msg := fmt.Sprintf("Added line %d to settings. Running tasks get it at their next request.", got.Line)
		answer(w, r, msg, map[string]int{"line": got.Line, "version": got.Version})
	case "dismiss":
		if err := s.deps.Blocked.Dismiss(r.Context(), id, domain.BlockedID(bid), by); err != nil {
			blockedError(w, r, err)
			return
		}
		answer(w, r, "Dismissed.", map[string]string{"decision": string(domain.Dismissed)})
	default:
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "The action is allow or dismiss."})
	}
}

func actionOf(w http.ResponseWriter, r *http.Request) string {
	if r.Header.Get("Content-Type") != "application/json" {
		return r.FormValue("action")
	}
	var in struct {
		Action string `json:"action"`
	}
	_ = json.NewDecoder(http.MaxBytesReader(w, r.Body, maxProxyBody)).Decode(&in)
	return in.Action
}

// answer gives a page a toast and anything else the body in JSON.
func answer(w http.ResponseWriter, r *http.Request, msg string, body any) {
	if !isHTMX(r) {
		writeJSON(w, http.StatusOK, body)
		return
	}
	w.Header().Set("HX-Reswap", "none")
	render(w, r, http.StatusOK, toastResponse(msg))
}

func blockedError(w http.ResponseWriter, r *http.Request, err error) {
	switch {
	case errors.Is(err, domain.ErrNotFound):
		http.NotFound(w, r)
	case errors.Is(err, domain.ErrRefused):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "A desk request needs a connection, not an allow line."})
	case errors.Is(err, domain.ErrStaleSettings):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "Settings changed while saving. Choose Allow again."})
	default:
		serverError(w, r, err)
	}
}
