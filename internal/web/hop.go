package web

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"

	"github.com/IniZio/moor/internal/domain"
)

// peerPerson carries the signed-in person on a hop between replicas, beside the shared secret.
const peerPerson = "X-Moor-Person"

const (
	maxScreenLines = 10000
	maxTyped       = 64 << 10
)

// noDeadline lifts the server's read and write timeouts from a connection that lives as long as a
// terminal does.
var noDeadline time.Time

// hostsOf turns trusted origins into the host patterns the websocket's Origin check takes.
func hostsOf(origins []string) []string {
	var hosts []string
	for _, o := range origins {
		if u, err := url.Parse(o); err == nil && u.Host != "" {
			hosts = append(hosts, u.Host)
		}
	}
	return hosts
}

var errNoPeer = fmt.Errorf("another portal replica holds the link and this one cannot reach it: %w", domain.ErrNoSession)

// internalRoutes are the hop targets another replica calls with the shared secret. They sit outside
// the person and cross-origin checks, which the replica the browser reached already made.
func (s *server) internalRoutes(r chi.Router) {
	r.Group(func(r chi.Router) {
		r.Use(s.peer)
		r.Get("/internal/terminal/{id}", s.localTerminal)
		r.Get("/internal/terminal/{id}/screen", s.local(s.screenHere))
		r.Post("/internal/terminal/{id}/input", s.local(s.inputHere))
	})
}

// peer admits a request that carries the secret the portal replicas share, as the person it names.
func (s *server) peer(next http.Handler) http.Handler {
	want := []byte("Bearer " + s.cfg.InternalSecret)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got := []byte(r.Header.Get("Authorization"))
		name := strings.TrimSpace(r.Header.Get(peerPerson))
		if s.cfg.InternalSecret == "" || subtle.ConstantTimeCompare(got, want) != 1 || !goodPerson(name) {
			http.Error(w, "not a portal replica", http.StatusUnauthorized)
			return
		}
		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), personKey{}, name)))
	})
}

func goodPerson(name string) bool {
	return name != "" && len(name) <= maxPersonLen && !strings.ContainsFunc(name, unicode.IsControl)
}

// hop passes the request to the replica that holds the link, as the signed-in person. The reverse
// proxy carries a websocket upgrade as well as a plain request.
func (s *server) hop(w http.ResponseWriter, r *http.Request, holder, path string) error {
	u, err := url.Parse(holder)
	if s.cfg.InternalSecret == "" || err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return errNoPeer
	}
	rc := http.NewResponseController(w)
	_, _ = rc.SetReadDeadline(noDeadline), rc.SetWriteDeadline(noDeadline)
	person := personOf(r.Context())
	p := &httputil.ReverseProxy{Rewrite: func(pr *httputil.ProxyRequest) {
		pr.Out.URL = &url.URL{Scheme: u.Scheme, Host: u.Host, Path: path, RawQuery: pr.In.URL.RawQuery}
		pr.Out.Host = u.Host
		pr.Out.Header.Set("Authorization", "Bearer "+s.cfg.InternalSecret)
		pr.Out.Header.Set(peerPerson, person)
	}}
	p.ServeHTTP(w, r)
	return nil
}

func (s *server) localTerminal(w http.ResponseWriter, r *http.Request) {
	id := taskID(r)
	_, here, err := s.deps.Terms.TermHolder(r.Context(), id)
	if err == nil && !here {
		err = fmt.Errorf("the link moved off this replica: %w", domain.ErrNoSession)
	}
	s.relay(w, r, id, err, &websocket.AcceptOptions{InsecureSkipVerify: true})
}

// routed runs here when this replica holds the task's link, and hops to the one that does otherwise.
func (s *server) routed(sub string, here func(http.ResponseWriter, *http.Request, domain.TaskID)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := taskID(r)
		holder, ok, err := s.deps.Terms.TermHolder(r.Context(), id)
		switch {
		case err != nil:
			termError(w, r, err)
		case ok:
			here(w, r, id)
		default:
			if err := s.hop(w, r, holder, "/internal/terminal/"+string(id)+sub); err != nil {
				termError(w, r, err)
			}
		}
	}
}

// local runs a hop target only while this replica still holds the link.
func (s *server) local(here func(http.ResponseWriter, *http.Request, domain.TaskID)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := taskID(r)
		_, ok, err := s.deps.Terms.TermHolder(r.Context(), id)
		switch {
		case err != nil:
			termError(w, r, err)
		case !ok:
			termError(w, r, fmt.Errorf("the link moved off this replica: %w", domain.ErrNoSession))
		default:
			here(w, r, id)
		}
	}
}

func termError(w http.ResponseWriter, r *http.Request, err error) {
	switch {
	case errors.Is(err, domain.ErrNotFound):
		http.NotFound(w, r)
	case errors.Is(err, domain.ErrNoSession):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "The task has no live session."})
	case errors.Is(err, domain.ErrRefused):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "This task can no longer be typed to."})
	default:
		serverError(w, r, err)
	}
}

// screenHere serves the session's screen as text, with lines of scrollback above it.
func (s *server) screenHere(w http.ResponseWriter, r *http.Request, id domain.TaskID) {
	lines, err := strconv.Atoi(r.URL.Query().Get("lines"))
	if err != nil || lines < 0 || lines > maxScreenLines {
		lines = 0
	}
	text, err := s.deps.Terms.Screen(r.Context(), id, lines)
	if err != nil {
		termError(w, r, err)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = w.Write([]byte(text))
}

// inputHere types a line into the session from the steering seat: it is refused while someone else
// holds the seat, and it reopens a done task first.
func (s *server) inputHere(w http.ResponseWriter, r *http.Request, id domain.TaskID) {
	var in struct {
		Text string `json:"text"`
	}
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxTyped))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&in); err != nil || strings.TrimSpace(in.Text) == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "Type something to send."})
		return
	}
	if h := s.seats.holder(id); h != "" && h != personOf(r.Context()) {
		writeJSON(w, http.StatusConflict, map[string]string{"error": h + " has the keyboard."})
		return
	}
	err := s.steer(r.Context(), id)
	if err == nil {
		err = s.deps.Terms.Type(r.Context(), id, in.Text)
	}
	if err != nil {
		termError(w, r, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
