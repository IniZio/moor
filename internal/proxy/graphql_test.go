package proxy

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

// listed is the default graphql_mutations list of the example settings.
var listed = []string{
	"createPullRequest", "updatePullRequest", "markPullRequestReadyForReview", "convertPullRequestToDraft", "addComment",
	"updateIssueComment", "addPullRequestReview", "submitPullRequestReview", "requestReviews", "addLabelsToLabelable",
	"removeLabelsFromLabelable", "createIssue", "updateIssue",
}

// ghBody is a request body as gh sends it: the query and its variables, nothing else.
func ghBody(t *testing.T, query string) string {
	t.Helper()
	b, err := json.Marshal(map[string]any{"query": query, "variables": map[string]any{"input": map[string]any{"pullRequestId": "PR_1"}}})
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestTheGraphQLJudge(t *testing.T) {
	for _, c := range judgeCases(t) {
		t.Run(c.name, func(t *testing.T) { c.check(t) })
	}
}

// judgeCases are real gh request shapes and the ways a request might hide a merge.
func judgeCases(t *testing.T) []judgeCase {
	return []judgeCase{
		{name: "gh pr view, a query", body: ghBody(t, `query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) {
			repository(owner: $owner, name: $repo) { pullRequest(number: $pr_number) { id, title, state, mergeable } } }`)},
		{name: "gh pr create", body: ghBody(t, `mutation PullRequestCreate($input:CreatePullRequestInput!){createPullRequest(input: $input){pullRequest{id,url}}}`)},
		{name: "gh pr edit, title and labels", body: ghBody(t, `mutation PullRequestUpdate($input:UpdatePullRequestInput!){
			updatePullRequest(input: $input){__typename} addLabelsToLabelable(input: {labelableId: "PR_1", labelIds: ["L_1"]}){__typename}}`)},
		{
			name: "gh pr merge", body: ghBody(t, `mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input: $input){clientMutationId}}`),
			reason: domain.DenyMerge, detail: "mergePullRequest",
		},
		{
			name: "gh pr merge --auto", body: ghBody(t, `mutation PullRequestAutoMerge($input:EnablePullRequestAutoMergeInput!){enablePullRequestAutoMerge(input: $input){clientMutationId}}`),
			reason: domain.DenyMerge, detail: "enablePullRequestAutoMerge",
		},
		{
			name: "a merge queue entry", body: ghBody(t, `mutation { enqueuePullRequest(input: {pullRequestId: "PR_1"}) { clientMutationId } }`),
			reason: domain.DenyMerge, detail: "enqueuePullRequest",
		},
		{
			name: "a merge mutation someone listed", body: ghBody(t, `mutation { mergeBranch(input: {}) { clientMutationId } }`),
			list: append([]string{"mergeBranch", "mergePullRequest"}, listed...), reason: domain.DenyMerge, detail: "mergeBranch",
		},
		{
			name: "a merge behind a harmless alias", body: ghBody(t, `mutation { addComment: mergePullRequest(input: {pullRequestId: "PR_1"}) { clientMutationId } }`),
			reason: domain.DenyMerge, detail: "mergePullRequest",
		},
		{name: "a listed mutation behind a merge-named alias", body: ghBody(t, `mutation { mergePullRequest: addComment(input: {subjectId: "PR_1", body: "hi"}) { clientMutationId } }`)},
		{
			name: "a merge in a fragment", body: ghBody(t, `mutation PullRequestCreate { ...F } fragment F on Mutation { mergePullRequest(input: {pullRequestId: "PR_1"}) { clientMutationId } }`),
			reason: domain.DenyMerge, detail: "mergePullRequest",
		},
		{
			name: "a merge in a nested inline fragment", body: ghBody(t, `mutation { ... on Mutation { ...G } } fragment G on Mutation { ... { mergePullRequest(input: {}) { clientMutationId } } }`),
			reason: domain.DenyMerge, detail: "mergePullRequest",
		},
		{name: "one fragment spread twice", body: ghBody(t, `mutation { ...F ...F } fragment F on Mutation { addComment(input: {subjectId: "PR_1", body: "x"}) { clientMutationId } }`)},
		{
			name: "a mutation off the list", body: ghBody(t, `mutation { deleteRef(input: {refId: "R_1"}) { clientMutationId } }`),
			reason: domain.DenyGraphQL, detail: "deleteRef",
		},
		{
			name: "no mutation passes without a list", body: ghBody(t, `mutation { addComment(input: {subjectId: "PR_1", body: "x"}) { clientMutationId } }`),
			list: []string{}, reason: domain.DenyGraphQL, detail: "addComment",
		},
		{name: "a batch", body: "[" + ghBody(t, "query { viewer { login } }") + "]", reason: domain.DenyGraphQL, detail: "batch"},
		{name: "a persisted query", body: `{"extensions":{"persistedQuery":{"version":1,"sha256Hash":"abc"}},"variables":{}}`, reason: domain.DenyGraphQL, detail: "envelope"},
		{name: "a query with an unknown key beside it", body: `{"query":"query { viewer { login } }","extensions":{}}`, reason: domain.DenyGraphQL, detail: "envelope"},
		{
			name: "two operations", body: ghBody(t, `query A { viewer { login } } mutation B { mergePullRequest(input: {}) { clientMutationId } }`),
			reason: domain.DenyGraphQL, detail: "one operation",
		},
		{name: "a subscription", body: ghBody(t, `subscription { issueUpdated { id } }`), reason: domain.DenyGraphQL, detail: "subscription"},
		{name: "a fragment cycle", body: ghBody(t, `mutation { ...F } fragment F on Mutation { ...F }`), reason: domain.DenyGraphQL, detail: "fragment"},
		{name: "a document that does not parse", body: ghBody(t, `mutation { mergePullRequest(`), reason: domain.DenyGraphQL, detail: "parse"},
		{name: "a body that is not JSON", body: "mutation { mergePullRequest }", reason: domain.DenyGraphQL, detail: "envelope"},
		{name: "two JSON values", body: ghBody(t, "query { viewer { login } }") + `{"query":"mutation { mergePullRequest }"}`, reason: domain.DenyGraphQL, detail: "envelope"},
		{name: "an oversized body", body: ghBody(t, "query { viewer { login } } # "+strings.Repeat("x", 64<<10)), reason: domain.DenyGraphQL, detail: "64 KiB"},
	}
}

type judgeCase struct {
	name   string
	body   string
	list   []string
	reason domain.DenyReason
	detail string
}

func (c judgeCase) check(t *testing.T) {
	t.Helper()
	list := c.list
	if list == nil {
		list = listed
	}
	reason, detail, ok := judgeGraphQL([]byte(c.body), list)
	switch {
	case c.reason == "" && !ok:
		t.Fatalf("refused (%s: %s), want it passed", reason, detail)
	case c.reason == "":
	case ok || reason != c.reason || !strings.Contains(detail, c.detail):
		t.Fatalf("got ok=%v %s %q, want %s naming %q", ok, reason, detail, c.reason, c.detail)
	case strings.Contains(detail, "PR_1") || strings.Contains(detail, "hi"):
		t.Fatalf("the reason %q carries a value", detail)
	}
}
