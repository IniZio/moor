package domain

type Recipe struct {
	Name   RecipeName
	Prompt string
	Skills []SkillName
	// Source is the integration whose events start the recipe. Empty means only a person starts it.
	Source TriggerSource
}
