package inbound

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/IniZio/moor/internal/domain"
)

// Source is the portal's own events: a schedule firing and a Run on a repository. Both have the group of their spec
// file and repository, so a spec's cancel-in-progress compares a firing with a Run.
const Source domain.EventSource = "portal"

// wallLayout writes a firing's minute in its schedule's zone. With the zone's name it is the firing's revision.
const wallLayout = "2006-01-02T15:04"

// Timer is the clock the schedules run on. Wait sleeps for d, and says false when ctx ended first.
type Timer interface {
	Now() time.Time
	Wait(ctx context.Context, d time.Duration) bool
}

// Loop fires the specs' schedules at the start of each minute until ctx ends. A wake-up late within the minute
// still fires it, and a minute that went by while the portal slept or was down is not fired later.
func (s *Service) Loop(ctx context.Context, clock Timer) {
	for {
		now := clock.Now()
		if !clock.Wait(ctx, now.Truncate(time.Minute).Add(time.Minute).Sub(now)) {
			return
		}
		if err := s.Tick(ctx, clock.Now()); err != nil {
			s.d.Log.Warn("schedules", "err", err)
		}
	}
}

// Tick fires each schedule entry that is due in the minute of now: one task for each repository its spec lists and
// Settings allows. Every replica ticks. A firing's key is its spec file, its repository and its minute in the
// entry's zone, and only the first replica to store the key starts the task. Keyed on the local minute, a minute
// that a clock change repeats fires once.
func (s *Service) Tick(ctx context.Context, now time.Time) error {
	files, err := s.d.Specs.TaskSpecs(ctx)
	if err != nil {
		return fmt.Errorf("task specs: %w", err)
	}
	allowed, err := s.d.Allowed.Allowed(ctx)
	if err != nil {
		return fmt.Errorf("allowed repositories: %w", err)
	}
	var errs []error
	for _, f := range files {
		for _, fire := range s.due(f, allowed, now) {
			if err := s.fire(ctx, fire); err != nil {
				errs = append(errs, err)
			}
		}
	}
	return errors.Join(errs...)
}

// firing is one schedule entry of a spec due on one repository, at local, its minute in the entry's zone.
type firing struct {
	file  domain.SpecFile
	repo  domain.Repo
	entry domain.ScheduleOn
	local time.Time
}

func (f firing) rev() domain.Revision {
	return domain.Revision(f.local.Format(wallLayout) + " " + f.local.Location().String())
}

func (s *Service) due(f domain.SpecFile, allowed []domain.Repo, now time.Time) []firing {
	if !f.Valid() {
		return nil
	}
	var out []firing
	for _, entry := range f.Spec.On.Schedule {
		local, ok := s.d.Crons.Fires(entry, now)
		if !ok {
			continue
		}
		for _, repo := range f.Spec.Repositories {
			if slices.Contains(allowed, repo) {
				out = append(out, firing{file: f, repo: repo, entry: entry, local: local})
			}
		}
	}
	return out
}

func (s *Service) fire(ctx context.Context, f firing) error {
	occ, err := s.repoEvent(ctx, f.file, f.repo, f.rev(), f.entry.Cron)
	if err != nil {
		return err
	}
	in := domain.Inbound{
		Source: Source, DeliveryID: string(occ.Event.Group) + "@" + string(f.rev()), Event: string(domain.EventSchedule), Action: f.entry.Cron, Occurrence: &occ,
	}
	_, err = s.receiveFor(ctx, in, func(context.Context, domain.Inbound) ([]domain.SpecFile, string, error) {
		return []domain.SpecFile{f.file}, "", nil
	})
	return err
}

// repoEvent is the event of a schedule firing, with its cron, or of a Run on a repository, with no cron. It names
// the repository in GitHub's own field names, so a prompt reads event.repository.full_name as it would in a
// workflow. Its default branch is left out when it cannot be read, and a prompt that reads it is then not filled.
func (s *Service) repoEvent(ctx context.Context, f domain.SpecFile, repo domain.Repo, rev domain.Revision, cron string) (domain.Occurrence, error) {
	var c repoContext
	c.Repository.FullName, c.Repository.HTMLURL, c.Schedule = string(repo.Path), "https://"+repo.String(), cron
	if s.d.Branches != nil {
		branch, err := s.d.Branches.DefaultBranch(ctx, repo)
		if err != nil {
			s.d.Log.Warn("default branch", "repo", repo, "err", err)
		}
		c.Repository.DefaultBranch = branch
	}
	raw, err := json.Marshal(c)
	if err != nil {
		return domain.Occurrence{}, fmt.Errorf("encode the event of %s: %w", repo, err)
	}
	name := domain.EventWorkflowDispatch
	if cron != "" {
		name = domain.EventSchedule
	}
	return domain.Occurrence{
		Incoming: domain.Incoming{Source: Source, Name: name, Action: cron, Repo: repo, Payload: raw},
		Event:    domain.Event{Source: Source, Group: domain.EventGroup(f.File + "@" + repo.String()), Rev: rev, Context: raw},
	}, nil
}

// repoContext is the JSON of the portal's own events, which the prompt templates and the screens read.
type repoContext struct {
	Repository struct {
		FullName      string `json:"full_name"`
		HTMLURL       string `json:"html_url"`
		DefaultBranch string `json:"default_branch,omitempty"`
	} `json:"repository"`
	Schedule string `json:"schedule,omitempty"`
}

// Reader reads the portal's own events for the screens. A schedule firing reads "schedule, Monday 06:00", its
// minute in its zone, and a Run on a repository has no words of its own. Both link the repository.
type Reader struct{}

func (Reader) ReadEvent(ev domain.Event) (domain.EventText, bool) {
	var c repoContext
	if ev.Source != Source || json.Unmarshal(ev.Context, &c) != nil {
		return domain.EventText{}, false
	}
	text := domain.EventText{URL: c.Repository.HTMLURL}
	if c.Schedule != "" {
		text.Subject = "schedule"
		wall, _, _ := strings.Cut(string(ev.Rev), " ")
		if at, err := time.Parse(wallLayout, wall); err == nil {
			text.Subject += ", " + at.Format("Monday 15:04")
		}
	}
	return text, true
}
