package yamlfile

import (
	"errors"
	"fmt"
	"strings"

	"go.yaml.in/yaml/v3"

	"github.com/IniZio/moor/internal/domain"
)

// ParseTaskSpec reads a task spec file into a domain.TaskSpec. Every problem comes back as ParseErrors
// with the line of the key or value at fault. A repository written as owner/name gets domain.DefaultRepoHost.
func ParseTaskSpec(data []byte) (domain.TaskSpec, error) {
	doc, err := load(data)
	if err != nil {
		return domain.TaskSpec{}, err
	}
	w := &walker{}
	spec := w.taskSpec(doc)
	if len(w.errs) > 0 {
		return domain.TaskSpec{}, w.errs
	}
	if err := spec.Validate(); err != nil {
		return domain.TaskSpec{}, fmt.Errorf("%w: %w", ErrInvalid, err)
	}
	return spec, nil
}

func (w *walker) taskSpec(doc *yaml.Node) domain.TaskSpec {
	var s domain.TaskSpec
	if doc == nil {
		w.failAt(1, "the file is empty")
		return s
	}
	o := w.object(doc.Content[0], "the file", []string{"name", "repositories", "on", "concurrency", "prompt", "review"})
	if o == nil {
		return s
	}
	if text, ok := w.text(w.required(o, "name"), "name"); ok {
		name, err := domain.ParseSpecName(text)
		if err != nil {
			w.fail(o.vals["name"], "%v", err)
		}
		s.Name = name
	}
	s.Repositories = w.specRepositories(o)
	s.On = w.on(w.required(o, "on"))
	s.Concurrency = w.concurrency(o)
	s.Prompt = w.prompt(w.required(o, "prompt"), "prompt")
	if n, ok := o.get("review"); ok {
		s.ReviewPrompt = w.prompt(n, "review")
	}
	return s
}

func (w *walker) specRepositories(o *object) []domain.Repo {
	n, ok := o.get("repositories")
	if !ok {
		return nil
	}
	repos := w.repositories(n)
	if len(repos) == 0 && len(w.errs) == 0 {
		w.fail(n, "repositories is empty: leave the key out to mean every allowed repository")
	}
	return repos
}

func (w *walker) concurrency(o *object) domain.Concurrency {
	n, ok := o.get("concurrency")
	if !ok {
		return domain.Concurrency{}
	}
	c := w.object(n, "concurrency", []string{"cancel-in-progress"})
	if c == nil {
		return domain.Concurrency{}
	}
	cancel, ok := c.get("cancel-in-progress")
	return domain.Concurrency{CancelInProgress: ok && w.flag(cancel, "cancel-in-progress")}
}

func (w *walker) on(n *yaml.Node) domain.On {
	var on domain.On
	o := w.object(n, "on", []string{"pull_request", "schedule", "workflow_dispatch"})
	if o == nil {
		return on
	}
	if len(o.vals) == 0 {
		w.fail(n, "on needs at least one event")
	}
	if v, ok := o.get("pull_request"); ok {
		on.PullRequest = w.pullRequest(v)
	}
	if v, ok := o.get("schedule"); ok {
		on.Schedule = w.schedule(v)
	}
	if v, ok := o.get("workflow_dispatch"); ok {
		on.WorkflowDispatch = w.dispatch(v)
	}
	return on
}

func (w *walker) pullRequest(n *yaml.Node) *domain.PullRequestOn {
	pr := &domain.PullRequestOn{}
	if isNull(n) {
		return pr
	}
	o := w.object(n, "pull_request", []string{"types"})
	if o == nil {
		return pr
	}
	typesNode, ok := o.get("types")
	if !ok {
		return pr
	}
	seen := map[domain.PullRequestType]bool{}
	items := w.list(typesNode, "types")
	if len(items) == 0 && len(w.errs) == 0 {
		w.fail(typesNode, "types is empty: leave the key out for the default types")
	}
	for _, item := range items {
		text, ok := w.text(item, "a pull_request type")
		if !ok {
			continue
		}
		typ, err := domain.ParsePullRequestType(text)
		switch {
		case err != nil:
			w.fail(item, "%v", err)
		case seen[typ]:
			w.fail(item, "pull_request type %s is listed twice", typ)
		default:
			seen[typ] = true
			pr.Types = append(pr.Types, typ)
		}
	}
	return pr
}

func (w *walker) schedule(n *yaml.Node) []domain.ScheduleOn {
	items := w.list(n, "schedule")
	if len(items) == 0 && len(w.errs) == 0 {
		w.fail(n, "schedule needs at least one cron")
	}
	var out []domain.ScheduleOn
	for _, item := range items {
		o := w.object(item, "a schedule entry", []string{"cron", "timezone"})
		text, ok := w.text(w.required(o, "cron"), "cron")
		if !ok {
			continue
		}
		if err := checkCron(text); err != nil {
			w.fail(o.vals["cron"], "%v", err)
			continue
		}
		out = append(out, domain.ScheduleOn{Cron: text, Timezone: w.timezone(o)})
	}
	return out
}

// timezone reads a schedule entry's optional time zone, an IANA name such as Europe/London.
func (w *walker) timezone(o *object) string {
	n, ok := o.get("timezone")
	if !ok {
		return ""
	}
	text, ok := w.text(n, "timezone")
	if !ok {
		return ""
	}
	if err := checkZone(text); err != nil {
		w.fail(n, "%v", err)
		return ""
	}
	return text
}

func (w *walker) dispatch(n *yaml.Node) *domain.DispatchOn {
	d := &domain.DispatchOn{}
	if isNull(n) {
		return d
	}
	o := w.object(n, "workflow_dispatch", []string{"inputs"})
	if o == nil {
		return d
	}
	if in, ok := o.get("inputs"); ok {
		d.Inputs = w.inputs(in)
	}
	return d
}

func (w *walker) inputs(n *yaml.Node) []domain.Input {
	o := w.object(n, "inputs", nil)
	if o == nil {
		return nil
	}
	var out []domain.Input
	for _, name := range o.order {
		if err := domain.CheckInputName(name); err != nil {
			w.fail(o.vals[name], "%v", err)
		}
		out = append(out, w.input(name, o.vals[name]))
	}
	return out
}

func (w *walker) input(name string, n *yaml.Node) domain.Input {
	in := domain.Input{Name: name}
	if isNull(n) {
		return in
	}
	o := w.object(n, "input "+name, []string{"description", "default", "required"})
	if o == nil {
		return in
	}
	if v, ok := o.get("description"); ok {
		in.Description, _ = w.text(v, "description")
	}
	if v, ok := o.get("default"); ok {
		in.Default = w.scalarText(v, "default")
	}
	if v, ok := o.get("required"); ok {
		in.Required = w.flag(v, "required")
	}
	return in
}

func (w *walker) scalarText(n *yaml.Node, what string) *string {
	if n.Kind != yaml.ScalarNode || isNull(n) {
		w.fail(n, "%s must be a single value", what)
		return nil
	}
	return &n.Value
}

// prompt reads a prompt or review text and checks its expressions. A template line maps to the file line of a
// literal block exactly, and to the line of the key's value for any other style.
func (w *walker) prompt(n *yaml.Node, what string) string {
	text, ok := w.text(n, what)
	if !ok {
		return ""
	}
	if what == "prompt" && strings.TrimSpace(text) == "" {
		w.fail(n, "prompt is empty")
	}
	var te *domain.TemplateError
	if err := domain.CheckTemplate(text); errors.As(err, &te) {
		line := n.Line + te.Line - 1
		if n.Style&(yaml.LiteralStyle|yaml.FoldedStyle) != 0 {
			line++
		}
		msg := te.Msg
		if te.Expr != "" {
			msg = "${{ " + te.Expr + " }}: " + msg
		}
		w.failAt(line, "%s: %s", what, msg)
	}
	return text
}
