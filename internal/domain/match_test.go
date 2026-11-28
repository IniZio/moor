package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

var (
	shopWeb = domain.Repo{Host: "github.com", Path: "example-org/shop-web"}
	shopAPI = domain.Repo{Host: "github.com", Path: "example-org/shop-api"}
)

func specOn(on domain.On, repos ...domain.Repo) domain.TaskSpec {
	return domain.TaskSpec{Name: "s", Repositories: repos, On: on, Prompt: "p"}
}

func TestSpecApplies(t *testing.T) {
	prTypes := domain.On{PullRequest: &domain.PullRequestOn{Types: []domain.PullRequestType{domain.PROpened, domain.PRClosed}}}
	prDefault := domain.On{PullRequest: &domain.PullRequestOn{}}
	cron := domain.On{Schedule: []domain.ScheduleOn{{Cron: "0 6 * * 1"}, {Cron: "30 7 * * *"}}}
	dispatch := domain.On{WorkflowDispatch: &domain.DispatchOn{}}
	all := domain.On{PullRequest: &domain.PullRequestOn{}, Schedule: cron.Schedule, WorkflowDispatch: dispatch.WorkflowDispatch}
	pr := func(action string, repo domain.Repo) domain.Incoming {
		return domain.Incoming{Source: "forge", Name: domain.EventPullRequest, Action: action, Repo: repo}
	}
	tick := func(c string) domain.Incoming {
		return domain.Incoming{Source: "clock", Name: domain.EventSchedule, Action: c, Repo: shopWeb}
	}
	run := domain.Incoming{Source: "person", Name: domain.EventWorkflowDispatch, Repo: shopWeb}
	cases := []struct {
		name string
		spec domain.TaskSpec
		in   domain.Incoming
		want bool
	}{
		{"listed type", specOn(prTypes), pr("opened", shopWeb), true},
		{"other listed type", specOn(prTypes), pr("closed", shopWeb), true},
		{"unlisted type", specOn(prTypes), pr("synchronize", shopWeb), false},
		{"unknown type", specOn(prTypes), pr("labeled", shopWeb), false},
		{"empty type", specOn(prTypes), pr("", shopWeb), false},
		{"default opened", specOn(prDefault), pr("opened", shopWeb), true},
		{"default synchronize", specOn(prDefault), pr("synchronize", shopWeb), true},
		{"default reopened", specOn(prDefault), pr("reopened", shopWeb), true},
		{"default is not ready_for_review", specOn(prDefault), pr("ready_for_review", shopWeb), false},
		{"default is not closed", specOn(prDefault), pr("closed", shopWeb), false},
		{"pull request not listed", specOn(cron), pr("opened", shopWeb), false},
		{"first cron", specOn(cron), tick("0 6 * * 1"), true},
		{"second cron", specOn(cron), tick("30 7 * * *"), true},
		{"cron with other spacing", specOn(cron), tick(" 0  6 * *  1"), true},
		{"other cron", specOn(cron), tick("0 7 * * 1"), false},
		{"no cron named", specOn(cron), tick(""), false},
		{"schedule not listed", specOn(dispatch), tick("0 6 * * 1"), false},
		{"dispatch listed", specOn(dispatch), run, true},
		{"dispatch not listed", specOn(cron), run, false},
		{"unknown event", specOn(all), domain.Incoming{Name: "push", Action: "opened", Repo: shopWeb}, false},
		{"no event name", specOn(all), domain.Incoming{Action: "opened", Repo: shopWeb}, false},
		{"repository listed", specOn(prTypes, shopWeb, shopAPI), pr("opened", shopAPI), true},
		{"repository not listed", specOn(prTypes, shopWeb), pr("opened", shopAPI), false},
		{"repository listed, type not", specOn(prTypes, shopWeb), pr("synchronize", shopWeb), false},
		{"no repositories means all", specOn(prTypes), pr("opened", shopAPI), true},
		{"repository of another host", specOn(prTypes, shopWeb), pr("opened", domain.Repo{Host: "forge.example", Path: shopWeb.Path}), false},
		{"dispatch outside the list", specOn(dispatch, shopWeb), domain.Incoming{Name: domain.EventWorkflowDispatch, Repo: shopAPI}, false},
		{"schedule inside the list", specOn(cron, shopWeb), tick("0 6 * * 1"), true},
		{"empty repository with a list", specOn(prTypes, shopWeb), pr("opened", domain.Repo{}), false},
	}
	for _, c := range cases {
		if got := c.spec.Applies(c.in); got != c.want {
			t.Errorf("%s: Applies = %v, want %v", c.name, got, c.want)
		}
	}
}

func TestValidateSpecSet(t *testing.T) {
	named := func(names ...string) []domain.TaskSpec {
		var out []domain.TaskSpec
		for _, n := range names {
			s := fullSpec()
			s.Name = domain.SpecName(n)
			out = append(out, s)
		}
		return out
	}
	for _, ok := range [][]domain.TaskSpec{nil, named("a"), named("PR evidence", "nightly", "PR evidence 2")} {
		if err := domain.ValidateSpecSet(ok); err != nil {
			t.Errorf("%v: %v", ok, err)
		}
	}
	for _, bad := range [][]domain.TaskSpec{named("a", "a"), named("PR evidence", "x", "pr EVIDENCE")} {
		if err := domain.ValidateSpecSet(bad); !errors.Is(err, domain.ErrBadTaskSpec) {
			t.Errorf("%v: err = %v", bad, err)
		}
	}
}

func TestRepositoriesNotAllowed(t *testing.T) {
	settings := domain.Settings{Repositories: []domain.Repo{shopWeb}}
	cases := []struct {
		name  string
		repos []domain.Repo
		set   domain.Settings
		want  []domain.Repo
	}{
		{"all allowed", []domain.Repo{shopWeb}, settings, nil},
		{"no list means every allowed repository", nil, settings, nil},
		{"one outside", []domain.Repo{shopWeb, shopAPI}, settings, []domain.Repo{shopAPI}},
		{"all outside, in order", []domain.Repo{shopAPI, shopWeb}, domain.Settings{}, []domain.Repo{shopAPI, shopWeb}},
	}
	for _, c := range cases {
		got := domain.RepositoriesNotAllowed(specOn(domain.On{WorkflowDispatch: &domain.DispatchOn{}}, c.repos...), c.set)
		if len(got) != len(c.want) {
			t.Errorf("%s: got %v, want %v", c.name, got, c.want)
			continue
		}
		for i := range got {
			if got[i] != c.want[i] {
				t.Errorf("%s: got %v, want %v", c.name, got, c.want)
			}
		}
	}
}
