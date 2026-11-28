package web

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
)

const maxBody = 2 << 20

type jsonProblem struct {
	Line    int    `json:"line"`
	Message string `json:"message"`
}

type jsonDiffLine struct {
	Kind string `json:"kind"`
	Text string `json:"text"`
}

type jsonVersion struct {
	Version int            `json:"version"`
	SavedBy string         `json:"saved_by"`
	SavedAt time.Time      `json:"saved_at"`
	Note    string         `json:"note"`
	Diff    []jsonDiffLine `json:"diff"`
}

func problemsJSON(ps []domain.SettingsProblem) []jsonProblem {
	out := make([]jsonProblem, len(ps))
	for i, p := range ps {
		out[i] = jsonProblem{Line: p.Line, Message: p.Message}
	}
	return out
}

func versionJSON(e domain.SettingsEntry) jsonVersion {
	kinds := map[domain.DiffKind]string{domain.DiffSame: "same", domain.DiffAdded: "added", domain.DiffRemoved: "removed"}
	diff := make([]jsonDiffLine, len(e.Diff))
	for i, l := range e.Diff {
		diff[i] = jsonDiffLine{Kind: kinds[l.Kind], Text: l.Text}
	}
	return jsonVersion{Version: e.N, SavedBy: e.SavedBy, SavedAt: e.SavedAt, Note: e.Note, Diff: diff}
}

func (s *server) getSettings(w http.ResponseWriter, r *http.Request) {
	cur, err := s.deps.Settings.Current(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"version": cur.N, "text": cur.Text})
}

// readSave reads the document a person typed, from a JSON body or from the form htmx sends.
func readSave(w http.ResponseWriter, r *http.Request) (domain.SettingsSave, bool) {
	r.Body = http.MaxBytesReader(w, r.Body, maxBody)
	in := domain.SettingsSave{By: personOf(r.Context())}
	if strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
		var body struct {
			Base int    `json:"base"`
			Text string `json:"text"`
			Note string `json:"note"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			http.Error(w, "the body is not the JSON of a settings save", http.StatusBadRequest)
			return in, false
		}
		in.Base, in.Text, in.Note = body.Base, body.Text, body.Note
		return in, true
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "the form could not be read", http.StatusBadRequest)
		return in, false
	}
	base, err := strconv.Atoi(r.PostForm.Get("base"))
	if err != nil || base < 0 {
		http.Error(w, "base is the version you started from, a number from 0", http.StatusBadRequest)
		return in, false
	}
	in.Base, in.Text, in.Note = base, r.PostForm.Get("text"), r.PostForm.Get("note")
	return in, true
}

func (s *server) putSettings(w http.ResponseWriter, r *http.Request) {
	in, ok := readSave(w, r)
	if !ok {
		return
	}
	saved, err := s.deps.Settings.Save(r.Context(), in)
	var invalid *domain.InvalidSettingsError
	var stale *domain.StaleSettingsError
	switch {
	case errors.As(err, &invalid):
		refused(w, r, invalid.Problems)
	case errors.As(err, &stale):
		staleSave(w, r, stale.Newest)
	case err != nil:
		serverError(w, r, err)
	default:
		savedOK(w, r, saved)
	}
}

func savedMessage(saved domain.SettingsSaved) string {
	if saved.Unchanged {
		return "No changes to save."
	}
	msg := fmt.Sprintf("Saved v%d. ", saved.Version.N)
	if len(saved.Replaced) > 0 {
		msg += "The new value of " + strings.Join(saved.Replaced, ", ") + " is stored; it shows as set. "
	}
	return msg + "Running tasks get it at their next request."
}

func savedOK(w http.ResponseWriter, r *http.Request, saved domain.SettingsSaved) {
	if !isHTMX(r) {
		writeJSON(w, http.StatusOK, map[string]any{"version": saved.Version.N, "unchanged": saved.Unchanged, "replaced": saved.Replaced})
		return
	}
	if saved.Unchanged {
		w.Header().Set("HX-Reswap", "none")
		render(w, r, http.StatusOK, toastResponse(savedMessage(saved)))
		return
	}
	w.Header().Set("HX-Trigger", "settings-saved")
	render(w, r, http.StatusOK, savedResponse(saved.Version.Text, saved.Version.N, savedMessage(saved)))
}

func refused(w http.ResponseWriter, r *http.Request, problems []domain.SettingsProblem) {
	if !isHTMX(r) {
		writeJSON(w, http.StatusUnprocessableEntity, map[string]any{"errors": problemsJSON(problems)})
		return
	}
	msg := "Not saved. Fix the lines marked in red."
	if lineList(problems) == "" {
		msg = "Not saved. Fix the problem listed under the editor."
	}
	w.Header().Set("HX-Reswap", "none")
	render(w, r, http.StatusUnprocessableEntity, refusedResponse(problems, msg))
}

func staleSave(w http.ResponseWriter, r *http.Request, newest int) {
	msg := fmt.Sprintf("Not saved. Someone saved v%d while you were editing. Copy your changes, reload the page and save again.", newest)
	if !isHTMX(r) {
		writeJSON(w, http.StatusConflict, map[string]any{"error": msg, "newest": newest})
		return
	}
	w.Header().Set("HX-Reswap", "none")
	render(w, r, http.StatusConflict, toastResponse(msg))
}

func (s *server) checkSettings(w http.ResponseWriter, r *http.Request) {
	in, ok := readSave(w, r)
	if !ok {
		return
	}
	problems := s.deps.Settings.Check(in.Text)
	if isHTMX(r) {
		render(w, r, http.StatusOK, errsList(problems, false))
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"errors": problemsJSON(problems)})
}

func (s *server) versions(w http.ResponseWriter, r *http.Request) {
	entries, err := s.deps.Settings.History(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	if isHTMX(r) {
		render(w, r, http.StatusOK, historyAside(entries, 0, false))
		return
	}
	out := make([]jsonVersion, len(entries))
	for i, e := range entries {
		out[i] = versionJSON(e)
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *server) version(w http.ResponseWriter, r *http.Request) {
	n, err := strconv.Atoi(chi.URLParam(r, "n"))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	entries, err := s.deps.Settings.History(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	for _, e := range entries {
		if e.N != n {
			continue
		}
		if isHTMX(r) {
			render(w, r, http.StatusOK, diffResponse(e, entries))
		} else {
			writeJSON(w, http.StatusOK, versionJSON(e))
		}
		return
	}
	http.NotFound(w, r)
}
