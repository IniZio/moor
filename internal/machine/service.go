package machine

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode"

	"github.com/IniZio/moor/internal/domain"
)

// ErrBadJoin is a join whose name or key cannot be stored.
var ErrBadJoin = errors.New("a join needs a machine id, a token, a host name and an ed25519 public key")

// JoinTTL is how long a join token lasts.
const JoinTTL = time.Hour

const maxName = 64

// Service enrols machines and runs their row actions. PortalURL is the address the join command dials.
type Service struct {
	store     Store
	tasks     Tasks
	clock     Clock
	portalURL string
}

func New(store Store, tasks Tasks, clock Clock, portalURL string) *Service {
	return &Service{store: store, tasks: tasks, clock: clock, portalURL: portalURL}
}

func hashToken(token string) []byte {
	sum := sha256.Sum256([]byte(token))
	return sum[:]
}

// Add makes a machine enrolled by the person and returns the join command, the only place its token
// ever shows.
func (s *Service) Add(ctx context.Context, by domain.Person) (domain.Enrolled, error) {
	now := s.clock.Now()
	id := domain.RunnerID("m-" + strings.ToLower(rand.Text()[:12]))
	token := rand.Text()
	add := domain.MachineAdd{ID: id, EnrolledBy: by, JoinHash: hashToken(token), JoinExpiresAt: now.Add(JoinTTL), CreatedAt: now}
	if err := s.store.AddMachine(ctx, add); err != nil {
		return domain.Enrolled{}, fmt.Errorf("add machine: %w", err)
	}
	m := domain.Machine{ID: id, EnrolledBy: by, CreatedAt: now}
	cmd := fmt.Sprintf("runner join -portal %s -id %s -token %s -key runner.key", s.portalURL, id, token)
	return domain.Enrolled{Machine: m, JoinCommand: cmd, ExpiresAt: add.JoinExpiresAt}, nil
}

// JoinInput is what a runner sends to join.
type JoinInput struct {
	ID     domain.RunnerID
	Token  string
	Name   string
	PubKey []byte
}

// Join burns the token and stores the runner's key and its host's name.
func (s *Service) Join(ctx context.Context, in JoinInput) error {
	name := strings.TrimSpace(in.Name)
	if in.ID == "" || in.Token == "" || name == "" || len(name) > maxName || strings.ContainsFunc(name, unicode.IsControl) || len(in.PubKey) != ed25519.PublicKeySize {
		return ErrBadJoin
	}
	j := domain.MachineJoin{ID: in.ID, JoinHash: hashToken(in.Token), Name: name, PubKey: in.PubKey, At: s.clock.Now()}
	if err := s.store.JoinMachine(ctx, j); err != nil {
		return fmt.Errorf("join: %w", err)
	}
	return nil
}

func (s *Service) Machine(ctx context.Context, id domain.RunnerID) (domain.Machine, error) {
	m, err := s.store.Machine(ctx, id)
	if err != nil {
		return domain.Machine{}, fmt.Errorf("machine: %w", err)
	}
	return m, nil
}

func (s *Service) Machines(ctx context.Context) ([]domain.Machine, error) {
	ms, err := s.store.Machines(ctx)
	if err != nil {
		return nil, fmt.Errorf("machines: %w", err)
	}
	return ms, nil
}

// Act runs a row action for the member who enrolled the machine and refuses anyone else. It returns the machine
// as it was and, for a revoke, how many tasks it failed.
func (s *Service) Act(ctx context.Context, id domain.RunnerID, by domain.Person, a domain.MachineAction) (domain.MachineActed, error) {
	m, err := s.store.Machine(ctx, id)
	if err != nil {
		return domain.MachineActed{}, fmt.Errorf("%s %s: %w", a, id, err)
	}
	if m.EnrolledBy != by {
		return domain.MachineActed{}, fmt.Errorf("%s %s: %w", a, id, domain.ErrNotEnroller)
	}
	acted := domain.MachineActed{Machine: m}
	now := s.clock.Now()
	switch a {
	case domain.Drain:
		err = s.store.DrainMachine(ctx, id, now)
	case domain.Resume:
		err = s.resume(ctx, m)
	case domain.Disconnect:
		err = s.store.DisconnectMachine(ctx, id)
	case domain.Revoke:
		acted.Failed, err = s.revoke(ctx, id, now)
	case domain.Remove:
		err = s.store.RemoveMachine(ctx, id)
	default:
		err = domain.ErrNotFound
	}
	if err != nil {
		return acted, fmt.Errorf("%s %s: %w", a, id, err)
	}
	return acted, nil
}

// resume puts a draining machine back in service. Clearing the drain twice is harmless, so two resumes
// that both read it as draining end in the same state.
func (s *Service) resume(ctx context.Context, m domain.Machine) error {
	if m.State(s.clock.Now()) != domain.MachineDraining {
		return domain.ErrNotDraining
	}
	if err := s.store.ResumeMachine(ctx, m.ID); err != nil {
		return fmt.Errorf("clear the drain: %w", err)
	}
	return nil
}

// revoke refuses the machine's key, which also ends its link, then fails the tasks it starts or runs and
// counts the ones it failed.
func (s *Service) revoke(ctx context.Context, id domain.RunnerID, now time.Time) (int, error) {
	if err := s.store.RevokeMachine(ctx, id, now); err != nil {
		return 0, fmt.Errorf("revoke: %w", err)
	}
	on, err := s.store.TasksOn(ctx, id)
	if err != nil {
		return 0, fmt.Errorf("tasks on the revoked machine: %w", err)
	}
	failed := 0
	var errs []error
	for _, t := range on {
		if t.State == domain.Delivering {
			continue
		}
		if _, err := s.tasks.MachineRevoked(ctx, t.ID); err != nil {
			errs = append(errs, err)
			continue
		}
		failed++
	}
	return failed, errors.Join(errs...)
}
