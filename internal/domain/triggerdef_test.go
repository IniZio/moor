package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func validDef() domain.TriggerDef {
	return domain.TriggerDef{
		Name: "pr-evidence", Source: "forge", Events: []domain.EventName{"opened", "pushed"},
		Prompt: "show {{.Title}}", Skills: []domain.SkillName{"drive"}, ReviewPrompt: "check it", Reply: true,
	}
}

func TestTriggerDefValidate(t *testing.T) {
	if err := validDef().Validate(); err != nil {
		t.Fatalf("a full definition: %v", err)
	}
	minimal := domain.TriggerDef{Name: "a", Source: "forge", Events: []domain.EventName{"opened"}, Prompt: "p"}
	if err := minimal.Validate(); err != nil {
		t.Errorf("a definition with no skills, review prompt or reply: %v", err)
	}
	bad := map[string]func(*domain.TriggerDef){
		"no name":          func(d *domain.TriggerDef) { d.Name = "" },
		"upper-case name":  func(d *domain.TriggerDef) { d.Name = "PR" },
		"name with a path": func(d *domain.TriggerDef) { d.Name = "a/b" },
		"leading dash":     func(d *domain.TriggerDef) { d.Name = "-a" },
		"no source":        func(d *domain.TriggerDef) { d.Source = "" },
		"no events":        func(d *domain.TriggerDef) { d.Events = nil },
		"empty event":      func(d *domain.TriggerDef) { d.Events = []domain.EventName{"opened", ""} },
		"repeated event":   func(d *domain.TriggerDef) { d.Events = []domain.EventName{"opened", "opened"} },
		"blank prompt":     func(d *domain.TriggerDef) { d.Prompt = " \n" },
		"empty skill":      func(d *domain.TriggerDef) { d.Skills = []domain.SkillName{""} },
		"repeated skill":   func(d *domain.TriggerDef) { d.Skills = []domain.SkillName{"drive", "drive"} },
	}
	for name, mutate := range bad {
		d := validDef()
		mutate(&d)
		if err := d.Validate(); !errors.Is(err, domain.ErrBadTriggerDef) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
}
