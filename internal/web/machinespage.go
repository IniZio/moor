package web

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"net/http"
	"slices"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
)

// machinesChanged is the event a row action or Add machine sends the page, so the rows refresh at once.
const machinesChanged = "machines-changed"

// menuByState is what the row menu offers for each state. Remove only follows a revoke.
var menuByState = map[domain.MachineState][]string{
	domain.MachineLinked:   {"Drain", "Disconnect", "Revoke"},
	domain.MachineDraining: {"Resume", "Disconnect", "Revoke"},
	domain.MachineOffline:  {"Revoke"},
	domain.MachineJoining:  {"Revoke"},
	domain.MachineRevoked:  {"Remove"},
}

// machineRowView is a machine as one row of the table shows it. Items is empty for a person who did not
// enrol the machine, who sees the row without a menu.
type machineRowView struct {
	ID    string
	Name  string
	Class string
	State string
	Seen  string
	Tasks int
	By    string
	Items []string
}

// machinesView is everything the Machines screen draws. Stamp is the state of the rows the page was drawn
// from, which the stream tells changes against.
type machinesView struct {
	Page  page
	Rows  []machineRowView
	Stamp string
}

func displayName(m domain.Machine) string {
	if m.Name == "" {
		return string(m.ID)
	}
	return m.Name
}

func machineViewOf(m domain.Machine, person string, now time.Time) machineRowView {
	state := m.State(now)
	v := machineRowView{ID: string(m.ID), Name: displayName(m), Class: string(state), State: string(state), Tasks: m.Running, By: string(m.EnrolledBy)}
	if state == domain.MachineLinked {
		v.State, v.Class = "online", "online"
	}
	if state == domain.MachineOffline && !m.HeardAt.IsZero() {
		v.Seen = "last seen " + ago(now, m.HeardAt)
	}
	if string(m.EnrolledBy) == person {
		v.Items = menuByState[state]
	}
	return v
}

func machineViews(ms []domain.Machine, person string, now time.Time) []machineRowView {
	out := make([]machineRowView, len(ms))
	for i, m := range ms {
		out[i] = machineViewOf(m, person, now)
	}
	return out
}

// stampOf names what the rows show, so it changes when a row would look different and not when only the
// clock moved.
func stampOf(rows []machineRowView) string {
	sum := sha256.Sum256(fmt.Appendf(nil, "%+v", rows))
	return hex.EncodeToString(sum[:8])
}

func (s *server) machineRows(ctx context.Context, person string) ([]machineRowView, error) {
	ms, err := s.deps.Machines.Machines(ctx)
	if err != nil {
		return nil, fmt.Errorf("list machines: %w", err)
	}
	return machineViews(ms, person, s.now()), nil
}

func (s *server) machinesPage(w http.ResponseWriter, r *http.Request) {
	rows, err := s.machineRows(r.Context(), personOf(r.Context()))
	if err != nil {
		serverError(w, r, err)
		return
	}
	render(w, r, http.StatusOK, machinesScreen(machinesView{Page: s.pageData(r), Rows: rows, Stamp: stampOf(rows)}))
}

// machineEvents serves GET /api/machines/events. since is the stamp the page was drawn with, so a change
// between drawing the page and opening the stream is told at once.
func (s *server) machineEvents(w http.ResponseWriter, r *http.Request) {
	sess, err := openStream(w, r)
	if err != nil {
		serverError(w, r, err)
		return
	}
	person := personOf(r.Context())
	stamp := func(ctx context.Context) (string, error) {
		rows, err := s.machineRows(ctx, person)
		return stampOf(rows), err
	}
	s.newFeed(sess, "machines", r.URL.Query().Get("since"), 0, stamp).run(r.Context(), "machine events")
}

// ownRow reads the row the path names for the signed-in person. A machine the person did not enrol is
// refused, since the menu and its questions are only theirs.
func (s *server) ownRow(w http.ResponseWriter, r *http.Request) (machineRowView, bool) {
	m, err := s.deps.Machines.Machine(r.Context(), domain.RunnerID(chi.URLParam(r, "id")))
	switch {
	case errors.Is(err, domain.ErrNotFound):
		http.NotFound(w, r)
		return machineRowView{}, false
	case err != nil:
		serverError(w, r, err)
		return machineRowView{}, false
	case string(m.EnrolledBy) != personOf(r.Context()):
		refusal(w, r, http.StatusForbidden, msgNotEnroller)
		return machineRowView{}, false
	}
	return machineViewOf(m, personOf(r.Context()), s.now()), true
}

// machineMenu answers with the actions cell of a row, which is how Keep puts the menu back.
func (s *server) machineMenu(w http.ResponseWriter, r *http.Request) {
	if v, ok := s.ownRow(w, r); ok {
		render(w, r, http.StatusOK, actionsCell(v))
	}
}

// askMachine answers with the question that replaces the menu, "Revoke build-1?" with Revoke and Keep.
func (s *server) askMachine(w http.ResponseWriter, r *http.Request) {
	v, ok := s.ownRow(w, r)
	if !ok {
		return
	}
	verb, known := askVerbs[r.URL.Query().Get("verb")]
	switch {
	case !known:
		http.NotFound(w, r)
	case !slices.Contains(v.Items, verb):
		refusal(w, r, http.StatusConflict, notOffered[verb])
	default:
		render(w, r, http.StatusOK, confirmCell(v, verb))
	}
}

// askVerbs are the menu items that ask first, by the word the question's route carries.
var askVerbs = map[string]string{"revoke": "Revoke", "remove": "Remove"}

// notOffered is what a person is told when they ask for an item that the machine's state does not offer.
var notOffered = map[string]string{"Revoke": "This machine is already revoked.", "Remove": msgNotRevoked}

// actedText says what a row action did, in the words of the Machines screen.
func actedText(a domain.MachineAction, acted domain.MachineActed) string {
	name := displayName(acted.Machine)
	switch a {
	case domain.Drain:
		return name + " is draining. Its running tasks finish. It takes no new ones."
	case domain.Resume:
		return name + " is taking tasks again."
	case domain.Disconnect:
		return "Link to " + name + " closed. It may reconnect at once. Its tasks keep running."
	case domain.Revoke:
		return revokedText(name, acted.Failed)
	case domain.Remove:
		return name + " removed."
	}
	return ""
}

func revokedText(name string, failed int) string {
	switch failed {
	case 0:
		return name + " revoked."
	case 1:
		return name + " revoked. Its 1 running task failed with the reason \"machine revoked\"."
	}
	return fmt.Sprintf("%s revoked. Its %d running tasks failed with the reason \"machine revoked\".", name, failed)
}
