package domain_test

import (
	"errors"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func fullSpec() domain.TaskSpec {
	return domain.TaskSpec{
		Name:         "pr-evidence",
		Repositories: []domain.Repo{{Host: "forge.example", Path: "acme/app"}, {Host: "forge.example", Path: "acme/api"}},
		On: domain.On{
			PullRequest:      &domain.PullRequestOn{Types: []domain.PullRequestType{domain.PROpened, domain.PRSynchronize}},
			Schedule:         []domain.ScheduleOn{{Cron: "0 9 * * 1"}},
			WorkflowDispatch: &domain.DispatchOn{},
		},
		Concurrency:  domain.Concurrency{CancelInProgress: true},
		Prompt:       "show {{.Title}}",
		ReviewPrompt: "check it",
	}
}

func TestTaskSpecValidate(t *testing.T) {
	if err := fullSpec().Validate(); err != nil {
		t.Fatalf("a full spec: %v", err)
	}
	for name, ok := range map[string]domain.TaskSpec{
		"only a pull request":      {Name: "a", On: domain.On{PullRequest: &domain.PullRequestOn{}}, Prompt: "p"},
		"only a schedule":          {Name: "a", On: domain.On{Schedule: []domain.ScheduleOn{{Cron: "*/5 * * * *"}}}, Prompt: "p"},
		"only a manual run":        {Name: "a", On: domain.On{WorkflowDispatch: &domain.DispatchOn{}}, Prompt: "p"},
		"cron with extra spaces":   {Name: "a", On: domain.On{Schedule: []domain.ScheduleOn{{Cron: " 0  9 * *  1 "}}}, Prompt: "p"},
		"a name as people write":   {Name: "PR evidence", On: domain.On{WorkflowDispatch: &domain.DispatchOn{}}, Prompt: "p"},
		"a name in another script": {Name: "審查 證據", On: domain.On{WorkflowDispatch: &domain.DispatchOn{}}, Prompt: "p"},
	} {
		if err := ok.Validate(); err != nil {
			t.Errorf("%s: %v", name, err)
		}
	}
	bad := map[string]func(*domain.TaskSpec){
		"no name":           func(s *domain.TaskSpec) { s.Name = "" },
		"space around name": func(s *domain.TaskSpec) { s.Name = " a" },
		"two-line name":     func(s *domain.TaskSpec) { s.Name = "a\nb" },
		"long name":         func(s *domain.TaskSpec) { s.Name = domain.SpecName(strings.Repeat("a", 81)) },
		"nothing in on":     func(s *domain.TaskSpec) { s.On = domain.On{} },
		"blank prompt":      func(s *domain.TaskSpec) { s.Prompt = " \n" },
		"unknown type":      func(s *domain.TaskSpec) { s.On.PullRequest.Types = []domain.PullRequestType{"merged"} },
		"empty type":        func(s *domain.TaskSpec) { s.On.PullRequest.Types = []domain.PullRequestType{""} },
		"repeated type": func(s *domain.TaskSpec) {
			s.On.PullRequest.Types = []domain.PullRequestType{domain.PROpened, domain.PROpened}
		},
		"repository without a host": func(s *domain.TaskSpec) { s.Repositories = []domain.Repo{{Path: "acme/app"}} },
		"repository without a path": func(s *domain.TaskSpec) { s.Repositories = []domain.Repo{{Host: "forge.example"}} },
		"repeated repository": func(s *domain.TaskSpec) {
			s.Repositories = []domain.Repo{{Host: "forge.example", Path: "acme/app"}, {Host: "forge.example", Path: "acme/app"}}
		},
		"cron of four":      func(s *domain.TaskSpec) { s.On.Schedule[0].Cron = "0 9 * *" },
		"cron of six":       func(s *domain.TaskSpec) { s.On.Schedule[0].Cron = "0 0 9 * * 1" },
		"empty cron":        func(s *domain.TaskSpec) { s.On.Schedule[0].Cron = "" },
		"cron out of range": func(s *domain.TaskSpec) { s.On.Schedule[0].Cron = "60 9 * * 1" },
		"repeated input": func(s *domain.TaskSpec) {
			s.On.WorkflowDispatch.Inputs = []domain.Input{{Name: "env"}, {Name: "env"}}
		},
		"input name with a dot": func(s *domain.TaskSpec) {
			s.On.WorkflowDispatch.Inputs = []domain.Input{{Name: "a.b"}}
		},
		"unsupported expression": func(s *domain.TaskSpec) { s.Prompt = "go ${{ secrets.x }}" },
		"unsupported review":     func(s *domain.TaskSpec) { s.ReviewPrompt = "go ${{ env.x }}" },
	}
	for name, mutate := range bad {
		s := fullSpec()
		mutate(&s)
		if err := s.Validate(); !errors.Is(err, domain.ErrBadTaskSpec) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
}
