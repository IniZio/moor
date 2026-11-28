package inbound

import (
	"context"
	"fmt"
	"slices"
	"strings"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/task"
)

// Run starts the task of one spec by hand, as the spec's own event would have. For a spec with a pull request event
// the input is a pull request link, and the task is the one its webhook would start. For any other spec it is a
// repository, written host/owner/name or owner/name, and the event is the one a schedule makes, less the cron. The
// spec states one difference: the duplicate key does not apply, so a Run on a commit that a task already covers
// starts another. The event's revision therefore also names the Run. It records the person as the one who started
// it, and lets the task use their own model token.
func (s *Service) Run(ctx context.Context, spec, input string, by domain.Person) (task.Outcome, error) {
	file, err := s.runnable(ctx, spec)
	if err != nil {
		return task.Outcome{}, err
	}
	occ, err := s.runEvent(ctx, file, strings.TrimSpace(input))
	if err != nil {
		return task.Outcome{}, err
	}
	if err := s.allowed(ctx, file.Spec, occ.Incoming.Repo); err != nil {
		return task.Outcome{}, err
	}
	prompts, err := file.Spec.Fill(occ.Incoming.Payload, nil)
	if err != nil {
		return task.Outcome{}, fmt.Errorf("fill the prompt: %w", err)
	}
	id, err := s.d.Numbers.NextTaskID(ctx)
	if err != nil {
		return task.Outcome{}, fmt.Errorf("task number: %w", err)
	}
	occ.Event.Rev = runRev(occ.Event.Rev, id)
	out, err := s.d.Tasks.StartByEvent(ctx, eventInput(id, &occ, file, prompts, by))
	if err != nil {
		return task.Outcome{}, fmt.Errorf("start %s: %w", spec, err)
	}
	s.d.Stop(context.WithoutCancel(ctx), out.Effects)
	return out, nil
}

// runEvent reads what a person gave Run into the event it stands for.
func (s *Service) runEvent(ctx context.Context, file domain.SpecFile, input string) (domain.Occurrence, error) {
	if file.Spec.On.PullRequest != nil {
		occ, err := s.d.Link.Occurrence(ctx, input)
		if err != nil {
			return occ, fmt.Errorf("read the link: %w", err)
		}
		return occ, nil
	}
	repo, err := domain.ParseRepo(input)
	if err != nil {
		return domain.Occurrence{}, fmt.Errorf("read the repository: %w", err)
	}
	return s.repoEvent(ctx, file, repo, "", "")
}

// runRev is the revision of a Run: the event's own, such as the head commit, with the Run's task after it.
func runRev(rev domain.Revision, id domain.TaskID) domain.Revision {
	if rev == "" {
		return domain.Revision("run-" + string(id))
	}
	return rev + domain.Revision("+run-"+string(id))
}

func (s *Service) runnable(ctx context.Context, name string) (domain.SpecFile, error) {
	files, err := s.d.Specs.TaskSpecs(ctx)
	if err != nil {
		return domain.SpecFile{}, fmt.Errorf("task specs: %w", err)
	}
	i := slices.IndexFunc(files, func(f domain.SpecFile) bool { return f.Valid() && string(f.Spec.Name) == name })
	switch {
	case i < 0:
		return domain.SpecFile{}, fmt.Errorf("%w: %q", domain.ErrNoSpec, name)
	case files[i].Spec.On.WorkflowDispatch == nil:
		return domain.SpecFile{}, fmt.Errorf("%w: %q", domain.ErrNotRunnable, name)
	}
	return files[i], nil
}

// allowed refuses a repository that Settings does not list, or that the spec's own repositories leave out.
func (s *Service) allowed(ctx context.Context, spec domain.TaskSpec, repo domain.Repo) error {
	listed, err := s.d.Allowed.Allowed(ctx)
	if err != nil {
		return fmt.Errorf("allowed repositories: %w", err)
	}
	if !slices.Contains(listed, repo) || (len(spec.Repositories) > 0 && !slices.Contains(spec.Repositories, repo)) {
		return &domain.NotAllowedError{Repo: repo}
	}
	return nil
}
