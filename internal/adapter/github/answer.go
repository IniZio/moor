package github

import (
	"context"
	"fmt"
	"net/url"
	"slices"
	"strconv"
	"strings"

	"github.com/google/go-github/v88/github"

	"github.com/IniZio/moor/internal/domain"
)

// TargetKind names what an answer to a pull request is: one comment on it.
const TargetKind = "github.pull_request_comment"

// Target says where the answer to a pull request event goes. An event that is not a pull request has none.
func (*Adapter) Target(ev domain.Event) (domain.DeliveryTarget, bool) {
	c, ok := prEventFrom(ev)
	if !ok || ev.Source != Source {
		return domain.DeliveryTarget{}, false
	}
	return domain.DeliveryTarget{Kind: TargetKind, Ref: string(ev.Group), Label: "pull request #" + strconv.Itoa(c.PullRequest.Number)}, true
}

// marker is the hidden line that opens the comment, so the product can find it again when it has lost its id.
func marker(t domain.DeliveryTarget) string { return "<!-- moor:answer " + t.Ref + " -->" }

func sectionMarker(spec domain.SpecName, s domain.Section) string {
	return "<!-- moor:section spec=" + url.QueryEscape(string(spec)) + " rev=" + url.QueryEscape(string(s.Rev)) + " -->"
}

// Body is the comment: one section per task spec, each opened by a hidden marker that carries the spec and the
// head commit, with that task's one line and a link to its result page.
func (*Adapter) Body(d domain.Delivery) string {
	var b strings.Builder
	b.WriteString(marker(domain.DeliveryTarget{Ref: d.Ref}) + "\n")
	specs := make([]domain.SpecName, 0, len(d.Sections))
	for spec := range d.Sections {
		specs = append(specs, spec)
	}
	slices.Sort(specs)
	for _, spec := range specs {
		s := d.Sections[spec]
		fmt.Fprintf(&b, "\n%s\n**%s** at `%s`: %s\n", sectionMarker(spec, s), spec, shortRev(s.Rev), oneLine(s.Line))
		if link := webURL(s.URL); link != "" {
			fmt.Fprintf(&b, "[Result page](%s)\n", link)
		}
	}
	return b.String()
}

func shortRev(r domain.Revision) string {
	if len(r) > 7 {
		return string(r[:7])
	}
	return string(r)
}

func oneLine(s string) string { return strings.Join(strings.Fields(s), " ") }

// webURL keeps an address the agent chose only when it is a web address, since the comment links it.
func webURL(raw string) string {
	u, err := url.Parse(raw)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return ""
	}
	return u.String()
}

// place splits a target's ref, owner/name#number, into its parts.
func place(t domain.DeliveryTarget) (owner, name string, number int, err error) {
	path, n, found := strings.Cut(t.Ref, "#")
	owner, name, ok := strings.Cut(path, "/")
	number, convErr := strconv.Atoi(n)
	if !found || !ok || convErr != nil {
		return "", "", 0, fmt.Errorf("%q is not owner/name#number", t.Ref)
	}
	return owner, name, number, nil
}

// withMarker finds the comment that opens with the marker of the place.
func withMarker(comments []*github.IssueComment, t domain.DeliveryTarget) (string, bool) {
	i := slices.IndexFunc(comments, func(c *github.IssueComment) bool { return strings.HasPrefix(c.GetBody(), marker(t)) })
	if i < 0 {
		return "", false
	}
	return strconv.FormatInt(comments[i].GetID(), 10), true
}

// Find looks for the comment the product made on the pull request before, by its marker.
func (a *Adapter) Find(ctx context.Context, t domain.DeliveryTarget) (string, bool, error) {
	owner, name, number, err := place(t)
	if err != nil {
		return "", false, err
	}
	c, err := a.forRepo(ctx, owner, name)
	if err != nil {
		return "", false, err
	}
	opts := &github.IssueListCommentsOptions{ListOptions: github.ListOptions{PerPage: 100}}
	for {
		comments, resp, err := c.Issues.ListComments(ctx, owner, name, number, opts)
		if err != nil {
			return "", false, fmt.Errorf("list comments of %s: %w", t.Ref, err)
		}
		if id, ok := withMarker(comments, t); ok {
			return id, true, nil
		}
		if resp.NextPage == 0 {
			return "", false, nil
		}
		opts.Page = resp.NextPage
	}
}

// Post makes the comment and returns its id.
func (a *Adapter) Post(ctx context.Context, t domain.DeliveryTarget, body string) (string, error) {
	owner, name, number, err := place(t)
	if err != nil {
		return "", err
	}
	c, err := a.forRepo(ctx, owner, name)
	if err != nil {
		return "", err
	}
	made, _, err := c.Issues.CreateComment(ctx, owner, name, number, &github.IssueComment{Body: &body})
	if err != nil {
		return "", fmt.Errorf("comment on %s: %w", t.Ref, err)
	}
	return strconv.FormatInt(made.GetID(), 10), nil
}

// Edit replaces the body of the comment Post made.
func (a *Adapter) Edit(ctx context.Context, t domain.DeliveryTarget, externalID, body string) error {
	owner, name, _, err := place(t)
	if err != nil {
		return err
	}
	id, err := strconv.ParseInt(externalID, 10, 64)
	if err != nil {
		return fmt.Errorf("comment id %q: %w", externalID, err)
	}
	c, err := a.forRepo(ctx, owner, name)
	if err != nil {
		return err
	}
	if _, _, err := c.Issues.EditComment(ctx, owner, name, id, &github.IssueComment{Body: &body}); err != nil {
		return fmt.Errorf("edit comment %d on %s: %w", id, t.Ref, err)
	}
	return nil
}
