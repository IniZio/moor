package githome_test

import (
	"context"
	"io/fs"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/githome"
	"github.com/IniZio/moor/internal/adapter/githome/githometest"
)

const repoPath = "example-org/team-home"

func commit(t *testing.T, s *githometest.Server, branch string, files map[string]string) string {
	t.Helper()
	hash, err := s.Commit(branch, files)
	if err != nil {
		t.Fatal(err)
	}
	return hash
}

func TestFetchReadsTheSpecsOfTheDefaultBranchTip(t *testing.T) {
	srv := githometest.New(t, repoPath, "tok")
	want := commit(t, srv, "main", map[string]string{"tasks/a.yaml": "name: a\n", "tasks/b.yaml": "name: b\n", "README.md": "x"})

	snap, err := githome.Fetch(context.Background(), githome.Remote{URL: srv.URL() + "/" + repoPath + ".git", Token: "tok"})
	if err != nil {
		t.Fatal(err)
	}
	if snap.Commit != want || snap.Branch != "main" {
		t.Fatalf("snapshot %s on %s, want %s on main", snap.Commit, snap.Branch, want)
	}
	got, err := fs.ReadFile(snap.Files, "tasks/a.yaml")
	if err != nil || string(got) != "name: a\n" {
		t.Fatalf("tasks/a.yaml = %q, %v", got, err)
	}
	if _, err := fs.Stat(snap.Files, "README.md"); err == nil {
		t.Fatal("a file outside tasks/ was kept")
	}
	for _, r := range srv.Requests() {
		if r.User != "x-access-token" || r.Password != "tok" {
			t.Fatalf("request %s came with %q:%q", r.Path, r.User, r.Password)
		}
	}
}

func TestFetchFollowsTheNamedBranch(t *testing.T) {
	srv := githometest.New(t, repoPath, "tok")
	commit(t, srv, "main", map[string]string{"tasks/a.yaml": "name: main\n"})
	want := commit(t, srv, "release", map[string]string{"tasks/a.yaml": "name: release\n"})

	snap, err := githome.Fetch(context.Background(), githome.Remote{URL: srv.URL() + "/" + repoPath + ".git", Branch: "release", Token: "tok"})
	if err != nil {
		t.Fatal(err)
	}
	got, _ := fs.ReadFile(snap.Files, "tasks/a.yaml")
	if snap.Commit != want || snap.Branch != "release" || string(got) != "name: release\n" {
		t.Fatalf("got %s on %s with %q", snap.Commit, snap.Branch, got)
	}
}

func TestFetchWithoutATasksFolderHoldsNoFiles(t *testing.T) {
	srv := githometest.New(t, repoPath, "tok")
	commit(t, srv, "main", map[string]string{"README.md": "x"})
	snap, err := githome.Fetch(context.Background(), githome.Remote{URL: srv.URL() + "/" + repoPath + ".git", Token: "tok"})
	if err != nil {
		t.Fatal(err)
	}
	entries, _ := fs.ReadDir(snap.Files, ".")
	if len(entries) != 0 {
		t.Fatalf("entries %v", entries)
	}
}

func TestFetchFailsPlainlyAndNeverNamesTheToken(t *testing.T) {
	srv := githometest.New(t, repoPath, "right-token")
	commit(t, srv, "main", map[string]string{"tasks/a.yaml": "name: a\n"})
	remote := githome.Remote{URL: srv.URL() + "/" + repoPath + ".git", Token: "wrong-token"}

	_, err := githome.Fetch(context.Background(), remote)
	if err == nil || strings.Contains(err.Error(), "wrong-token") {
		t.Fatalf("a wrong token gave %v", err)
	}
	srv.Fail(1)
	remote.Token = "right-token"
	if _, err := githome.Fetch(context.Background(), remote); err == nil {
		t.Fatal("a server error was not reported")
	}
	if _, err := githome.Fetch(context.Background(), remote); err != nil {
		t.Fatalf("after the failure: %v", err)
	}
}
