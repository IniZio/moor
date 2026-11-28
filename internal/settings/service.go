package settings

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// LineError is one problem in a typed document. Line counts from 1 and is 0 when the problem has no line.
type LineError struct {
	Line    int
	Message string
}

// InvalidError lists every problem of a refused document.
type InvalidError struct{ Lines []LineError }

func (e *InvalidError) Error() string {
	parts := make([]string, len(e.Lines))
	for i, l := range e.Lines {
		parts[i] = fmt.Sprintf("line %d: %s", l.Line, l.Message)
	}
	return "invalid settings: " + strings.Join(parts, "; ")
}

// StaleError is a save based on a version that is no longer the newest.
type StaleError struct{ Newest int }

func (e *StaleError) Error() string {
	return fmt.Sprintf("settings changed: the newest version is v%d", e.Newest)
}

func (e *StaleError) Is(target error) bool { return target == domain.ErrStaleSettings }

// InitialText is what the editor shows until the first save.
const InitialText = `# Settings. Everyone in the organisation can read and edit this document.
# repositories: where tasks run, and what agents may clone.
# A value shows only as set or not set. Type a new one and save to replace it.
repositories: []
allow: []
connections: []
`

type Service struct {
	store  Store
	parser Parser
	sealer Sealer
	clock  Clock
}

func New(store Store, parser Parser, sealer Sealer, clock Clock) *Service {
	return &Service{store: store, parser: parser, sealer: sealer, clock: clock}
}

// Current is the newest version, or version 0 with InitialText before the first save.
func (s *Service) Current(ctx context.Context) (domain.SettingsVersion, error) {
	v, err := s.store.LatestSettings(ctx)
	if errors.Is(err, domain.ErrNotFound) {
		return domain.SettingsVersion{Text: InitialText}, nil
	}
	if err != nil {
		return domain.SettingsVersion{}, fmt.Errorf("load settings: %w", err)
	}
	return v, nil
}

// Check reports the problems of a document without saving it.
func (s *Service) Check(text string) []LineError {
	_, err := s.parser.Parse(text)
	var invalid *InvalidError
	if errors.As(err, &invalid) {
		return invalid.Lines
	}
	return nil
}

// Save is a whole document typed by a person, based on version Base.
type Save struct {
	Base int
	Text string
	Note string
	By   string
}

// Saved is what a save did. Unchanged means the document and its values matched the newest version, so no
// version was made. Replaced names the connections whose value was typed.
type Saved struct {
	Version   domain.SettingsVersion
	Unchanged bool
	Replaced  []string
}

// Save stores the document as a new version. Typed secret values are sealed and stored apart from it.
func (s *Service) Save(ctx context.Context, in Save) (Saved, error) {
	cur, err := s.Current(ctx)
	if err != nil {
		return Saved{}, err
	}
	if cur.N != in.Base {
		return Saved{}, &StaleError{Newest: cur.N}
	}
	parsed, err := s.parser.Parse(in.Text)
	if err != nil {
		return Saved{}, fmt.Errorf("check settings: %w", err)
	}
	if cur.N > 0 && parsed.Text == cur.Text && len(parsed.Secrets) == 0 {
		return Saved{Version: cur, Unchanged: true}, nil
	}
	sealed, err := s.seal(parsed.Secrets)
	if err != nil {
		return Saved{}, err
	}
	replaced := make([]string, len(parsed.Secrets))
	for i, sec := range parsed.Secrets {
		replaced[i] = sec.Connection
	}
	note := in.Note
	if note == "" && len(replaced) > 0 {
		note = "replaced the value of " + strings.Join(replaced, ", ")
	}
	v, err := s.store.SaveSettings(ctx, domain.SettingsWrite{
		Base: in.Base, Text: parsed.Text, Note: note, By: in.By, At: s.clock.Now(), Keep: parsed.Connections, Secrets: sealed,
	})
	if err != nil {
		return Saved{}, s.staleOrErr(ctx, err)
	}
	return Saved{Version: v, Replaced: replaced}, nil
}

func (s *Service) seal(secrets []domain.NewSecret) ([]domain.SealedSecret, error) {
	now := s.clock.Now()
	out := make([]domain.SealedSecret, 0, len(secrets))
	for _, sec := range secrets {
		ciphertext, keyID, err := s.sealer.Seal([]byte(sec.Value))
		if err != nil {
			return nil, fmt.Errorf("seal the value of %q: %w", sec.Connection, err)
		}
		out = append(out, domain.SealedSecret{Connection: sec.Connection, Ciphertext: ciphertext, KeyID: keyID, UpdatedAt: now})
	}
	return out, nil
}

// staleOrErr names the newest version when the store refused a stale base, so the person can reload.
func (s *Service) staleOrErr(ctx context.Context, err error) error {
	if !errors.Is(err, domain.ErrStaleSettings) {
		return err
	}
	cur, curErr := s.Current(ctx)
	if curErr != nil {
		return err
	}
	return &StaleError{Newest: cur.N}
}
