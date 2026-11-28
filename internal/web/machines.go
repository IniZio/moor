package web

import (
	"context"
	"errors"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
)

// MachineService enrols machines and runs their row actions.
type MachineService interface {
	Machines(ctx context.Context) ([]domain.Machine, error)
	Machine(ctx context.Context, id domain.RunnerID) (domain.Machine, error)
	Add(ctx context.Context, by domain.Person) (domain.Enrolled, error)
	Act(ctx context.Context, id domain.RunnerID, by domain.Person, a domain.MachineAction) (domain.MachineActed, error)
}

type jsonMachine struct {
	ID         string     `json:"id"`
	Name       string     `json:"name"`
	State      string     `json:"state"`
	Running    int        `json:"running"`
	EnrolledBy string     `json:"enrolled_by"`
	Build      string     `json:"build"`
	HeardAt    *time.Time `json:"heard_at"`
	Controls   bool       `json:"controls"`
}

func (s *server) jsonMachineOf(r *http.Request, m domain.Machine) jsonMachine {
	out := jsonMachine{
		ID: string(m.ID), Name: m.Name, State: string(m.State(s.now())), Running: m.Running,
		EnrolledBy: string(m.EnrolledBy), Build: m.Build, Controls: string(m.EnrolledBy) == personOf(r.Context()),
	}
	if !m.HeardAt.IsZero() {
		out.HeardAt = &m.HeardAt
	}
	return out
}

// listMachines answers a page with the rows and anything else with the machines in JSON.
func (s *server) listMachines(w http.ResponseWriter, r *http.Request) {
	ms, err := s.deps.Machines.Machines(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	if isHTMX(r) {
		w.Header().Add("Vary", "HX-Request")
		render(w, r, http.StatusOK, machineRows(machineViews(ms, personOf(r.Context()), s.now())))
		return
	}
	out := make([]jsonMachine, len(ms))
	for i, m := range ms {
		out[i] = s.jsonMachineOf(r, m)
	}
	w.Header().Add("Vary", "HX-Request")
	writeJSON(w, http.StatusOK, map[string]any{"machines": out})
}

// addMachine answers a page with the box that shows the join command, and anything else with the machine and
// the command in JSON. Either way it is the one place the command shows.
func (s *server) addMachine(w http.ResponseWriter, r *http.Request) {
	e, err := s.deps.Machines.Add(r.Context(), domain.Person(personOf(r.Context())))
	if err != nil {
		serverError(w, r, err)
		return
	}
	if isHTMX(r) {
		w.Header().Set("HX-Trigger", machinesChanged)
		render(w, r, http.StatusCreated, joinBox(e.JoinCommand, e.ExpiresAt.UTC().Format("15:04")))
		return
	}
	writeJSON(w, http.StatusCreated, map[string]any{"machine": s.jsonMachineOf(r, e.Machine), "join_command": e.JoinCommand})
}

var rowActions = map[string]domain.MachineAction{"drain": domain.Drain, "disconnect": domain.Disconnect, "revoke": domain.Revoke}

func (s *server) actOnMachine(w http.ResponseWriter, r *http.Request) {
	a, ok := rowActions[chi.URLParam(r, "action")]
	if !ok {
		http.NotFound(w, r)
		return
	}
	s.actMachine(w, r, a)
}

func (s *server) removeMachine(w http.ResponseWriter, r *http.Request) {
	s.actMachine(w, r, domain.Remove)
}

const (
	msgNotEnroller = "Only the member who added this machine can change it."
	msgNotRevoked  = "Revoke the machine before you remove it."
)

func (s *server) actMachine(w http.ResponseWriter, r *http.Request, a domain.MachineAction) {
	acted, err := s.deps.Machines.Act(r.Context(), domain.RunnerID(chi.URLParam(r, "id")), domain.Person(personOf(r.Context())), a)
	switch {
	case err == nil:
		actedReply(w, r, a, acted)
	case errors.Is(err, domain.ErrNotFound):
		http.NotFound(w, r)
	case errors.Is(err, domain.ErrNotEnroller):
		refusal(w, r, http.StatusForbidden, msgNotEnroller)
	case errors.Is(err, domain.ErrNotRevoked):
		refusal(w, r, http.StatusConflict, msgNotRevoked)
	default:
		serverError(w, r, err)
	}
}

// actedReply answers a page with a toast and a nudge to refresh the rows, and anything else with no content.
func actedReply(w http.ResponseWriter, r *http.Request, a domain.MachineAction, acted domain.MachineActed) {
	if !isHTMX(r) {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	w.Header().Set("HX-Reswap", "none")
	w.Header().Set("HX-Trigger", machinesChanged)
	render(w, r, http.StatusOK, toastResponse(actedText(a, acted)))
}

// refusal answers a page with a toast that leaves the page as it is, and anything else with the words.
func refusal(w http.ResponseWriter, r *http.Request, status int, msg string) {
	if !isHTMX(r) {
		http.Error(w, msg, status)
		return
	}
	w.Header().Set("HX-Reswap", "none")
	render(w, r, status, toastResponse(msg))
}
