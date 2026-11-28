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
	Add(ctx context.Context, by domain.Person) (domain.Enrolled, error)
	Act(ctx context.Context, id domain.RunnerID, by domain.Person, a domain.MachineAction) error
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

func (s *server) listMachines(w http.ResponseWriter, r *http.Request) {
	ms, err := s.deps.Machines.Machines(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	out := make([]jsonMachine, len(ms))
	for i, m := range ms {
		out[i] = s.jsonMachineOf(r, m)
	}
	writeJSON(w, http.StatusOK, map[string]any{"machines": out})
}

func (s *server) addMachine(w http.ResponseWriter, r *http.Request) {
	e, err := s.deps.Machines.Add(r.Context(), domain.Person(personOf(r.Context())))
	if err != nil {
		serverError(w, r, err)
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

func (s *server) actMachine(w http.ResponseWriter, r *http.Request, a domain.MachineAction) {
	err := s.deps.Machines.Act(r.Context(), domain.RunnerID(chi.URLParam(r, "id")), domain.Person(personOf(r.Context())), a)
	switch {
	case err == nil:
		w.WriteHeader(http.StatusNoContent)
	case errors.Is(err, domain.ErrNotFound):
		http.NotFound(w, r)
	case errors.Is(err, domain.ErrNotEnroller):
		http.Error(w, "Only the member who added this machine can change it.", http.StatusForbidden)
	case errors.Is(err, domain.ErrNotRevoked):
		http.Error(w, "Revoke the machine before you remove it.", http.StatusConflict)
	default:
		serverError(w, r, err)
	}
}
