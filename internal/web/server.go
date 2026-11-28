package web

import (
	"context"
	"embed"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"

	"github.com/IniZio/moor/internal/domain"
)

//go:embed static
var staticFiles embed.FS

// Config holds what an operator sets. UserHeader names the request header that carries the signed-in person.
type Config struct {
	UserHeader     string
	OrgName        string
	TrustedOrigins []string
	// InternalSecret is shared by the portal replicas, which send it on a terminal hop. Without it no
	// replica hops and none takes a hop.
	InternalSecret string
}

// SettingsService is what the settings screen needs.
type SettingsService interface {
	Current(ctx context.Context) (domain.SettingsVersion, error)
	Check(text string) []domain.SettingsProblem
	Save(ctx context.Context, in domain.SettingsSave) (domain.SettingsSaved, error)
	History(ctx context.Context) ([]domain.SettingsEntry, error)
	// Allowed lists the repositories the newest document allows.
	Allowed(ctx context.Context) ([]domain.Repo, error)
}

// Deps are the parts the server calls. Ready reports whether the database answers.
type Deps struct {
	Settings SettingsService
	Specs    SpecSource
	Start    TaskStarter
	Act      TaskActions
	Tasks    TaskReader
	Machines MachineService
	Terms    Terminals
	Ready    func(ctx context.Context) error
	// Now is the clock the ages and durations read, and is time.Now when left nil.
	Now func() time.Time
}

type server struct {
	cfg         Config
	deps        Deps
	seats       seats
	originHosts []string
}

// New builds the portal's handler. Every route but the health checks and the static files needs a person.
func New(cfg Config, deps Deps) (http.Handler, error) {
	if cfg.UserHeader == "" {
		return nil, errors.New("web: the name of the user header is not set")
	}
	guard := http.NewCrossOriginProtection()
	for _, origin := range cfg.TrustedOrigins {
		if err := guard.AddTrustedOrigin(origin); err != nil {
			return nil, fmt.Errorf("web: trusted origin %q: %w", origin, err)
		}
	}
	static, err := fs.Sub(staticFiles, "static")
	if err != nil {
		return nil, fmt.Errorf("web: static files: %w", err)
	}
	s := &server{cfg: cfg, deps: deps, originHosts: hostsOf(cfg.TrustedOrigins)}
	r := chi.NewRouter()
	r.Use(middleware.RequestID, middleware.Logger, middleware.Recoverer)
	r.Get("/healthz", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("ok\n")) })
	r.Get("/readyz", s.readyz)
	r.Handle("/static/*", http.StripPrefix("/static/", cached(http.FileServerFS(static))))
	s.internalRoutes(r)
	r.Group(func(r chi.Router) {
		r.Use(guard.Handler, s.person)
		r.Get("/", func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, "/tasks", http.StatusFound) })
		r.Get("/tasks", s.tasksPage)
		r.Get("/tasks/{id}", s.runPage)
		r.Get("/tasks/{id}/{part}", s.runPage)
		r.Get("/machines", s.machinesPage)
		r.Get("/settings", s.settingsPage)
		r.Get("/api/me", s.me)
		r.Get("/api/task-specs", s.taskSpecs)
		r.Get("/api/tasks", s.listTasks)
		r.Post("/api/tasks", s.startTask)
		r.Get("/api/tasks/events", s.taskListEvents)
		r.Get("/api/tasks/{id}", s.taskSnapshot)
		r.Get("/api/tasks/{id}/events", s.runEvents)
		r.Get("/api/tasks/{id}/terminal", s.terminal)
		r.Get("/api/tasks/{id}/screen", s.routed("/screen", s.screenHere))
		r.Post("/api/tasks/{id}/input", s.routed("/input", s.inputHere))
		r.Post("/api/tasks/{id}/stop", s.stopTask)
		r.Post("/api/tasks/{id}/end", s.endTask)
		r.Post("/api/tasks/{id}/retry", s.retryTask)
		r.Get("/api/machines", s.listMachines)
		r.Get("/api/machines/events", s.machineEvents)
		r.Get("/api/machines/{id}/ask", s.askMachine)
		r.Get("/api/machines/{id}/menu", s.machineMenu)
		r.Post("/api/machines", s.addMachine)
		r.Post("/api/machines/{id}/{action}", s.actOnMachine)
		r.Delete("/api/machines/{id}", s.removeMachine)
		r.Get("/api/settings", s.getSettings)
		r.Put("/api/settings", s.putSettings)
		r.Post("/api/settings/check", s.checkSettings)
		r.Get("/api/settings/versions", s.versions)
		r.Get("/api/settings/versions/{n}", s.version)
	})
	return r, nil
}

func cached(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-cache")
		next.ServeHTTP(w, r)
	})
}

func (s *server) readyz(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
	defer cancel()
	if err := s.deps.Ready(ctx); err != nil {
		http.Error(w, "not ready", http.StatusServiceUnavailable)
		return
	}
	_, _ = w.Write([]byte("ok\n"))
}
