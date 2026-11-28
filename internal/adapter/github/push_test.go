package github_test

import (
	"context"
	"encoding/json"
	"net/http"
	"reflect"
	"testing"

	"github.com/IniZio/moor/internal/adapter/github/githubtest"
	"github.com/IniZio/moor/internal/domain"
)

type pushed struct {
	repo   domain.Repo
	branch string
}

type pushes struct{ got []pushed }

func (p *pushes) Pushed(repo domain.Repo, branch string) { p.got = append(p.got, pushed{repo, branch}) }

func pushPayload(ref, repo string, deleted bool) []byte {
	b, _ := json.Marshal(map[string]any{"ref": ref, "deleted": deleted, "repository": map[string]any{"full_name": repo}})
	return b
}

func TestAPushToABranchIsToldToThoseWhoAskedForPushes(t *testing.T) {
	r := newRig(t)
	p := &pushes{}
	h := r.adapter.HookFor(r.sink, p)
	rec := deliver(h, "push", "d-1", pushPayload("refs/heads/main", "Example-Org/Team-Home", false), secret)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("status %d", rec.Code)
	}
	want := []pushed{{domain.Repo{Host: "github.com", Path: "example-org/team-home"}, "main"}}
	if !reflect.DeepEqual(p.got, want) || len(r.sink.got) != 0 {
		t.Fatalf("pushes %+v, sink %v", p.got, r.sink.got)
	}
}

func TestAPushThatIsNotToABranchIsSetAside(t *testing.T) {
	r := newRig(t)
	p := &pushes{}
	h := r.adapter.HookFor(r.sink, p)
	for name, body := range map[string][]byte{
		"tag":            pushPayload("refs/tags/v1", "example-org/team-home", false),
		"deleted branch": pushPayload("refs/heads/old", "example-org/team-home", true),
		"no repository":  []byte("{}"),
	} {
		if rec := deliver(h, "push", "d-"+name, body, secret); rec.Code != http.StatusAccepted {
			t.Errorf("%s: status %d", name, rec.Code)
		}
	}
	if len(p.got) != 0 {
		t.Fatalf("told of %+v", p.got)
	}
}

func TestAPushWithTheWrongSignatureIsNotTold(t *testing.T) {
	r := newRig(t)
	p := &pushes{}
	h := r.adapter.HookFor(r.sink, p)
	if rec := deliver(h, "push", "d-1", pushPayload("refs/heads/main", "example-org/team-home", false), "another-secret"); rec.Code != http.StatusUnauthorized || len(p.got) != 0 {
		t.Fatalf("status %d, told %+v", rec.Code, p.got)
	}
}

func TestAReadTokenIsLimitedToOneRepositoryAndToReading(t *testing.T) {
	r := newRig(t)
	token, _, err := r.adapter.ReadToken(context.Background(), "example-org/team-home")
	if err != nil || token == "" {
		t.Fatalf("token %q: %v", token, err)
	}
	want := []githubtest.TokenRequest{{Installation: "4242", Repositories: []string{"team-home"}, Permissions: map[string]string{"contents": "read"}}}
	if got := r.fake.TokenRequests(); !reflect.DeepEqual(got, want) {
		t.Fatalf("token requests %+v, want %+v", got, want)
	}
}
