package yamlfile_test

import (
	"context"
	"errors"
	"io/fs"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
)

func specText(name string) string {
	return "name: " + name + "\non:\n  workflow_dispatch:\nprompt: p\n"
}

func TestHomeListsTheSpecsAndKeepsTheBrokenOnesWithTheirError(t *testing.T) {
	home := yamlfile.NewHome(fstest.MapFS{
		"tasks/b-check.yaml":    {Data: []byte(specText("Dependency check"))},
		"tasks/a-evidence.yaml": {Data: []byte(specText("PR evidence"))},
		"tasks/c-broken.yaml":   {Data: []byte("name: c\non:\n  push:\nprompt: p\n")},
		"tasks/d-copy.yaml":     {Data: []byte(specText("pr EVIDENCE"))},
		"tasks/notes.md":        {Data: []byte("not a spec")},
		"tasks/sub/e.yaml":      {Data: []byte(specText("Nested"))},
		"README.md":             {Data: []byte("# home")},
	})
	got, err := home.TaskSpecs(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	var files []string
	for _, f := range got {
		files = append(files, f.File)
	}
	if want := "tasks/a-evidence.yaml tasks/b-check.yaml tasks/c-broken.yaml tasks/d-copy.yaml"; strings.Join(files, " ") != want {
		t.Fatalf("files = %v, want %s", files, want)
	}
	if !got[0].Valid() || got[0].Spec.Name != "PR evidence" || !got[1].Valid() || got[1].Spec.Name != "Dependency check" {
		t.Errorf("valid specs = %+v, %+v", got[0], got[1])
	}
	if got[2].Valid() || !strings.Contains(got[2].Problem, "line 3") || !strings.Contains(got[2].Problem, "push") || strings.Contains(got[2].Problem, "invalid file") {
		t.Errorf("broken spec problem = %q", got[2].Problem)
	}
	if got[3].Valid() || !strings.Contains(got[3].Problem, "tasks/a-evidence.yaml") {
		t.Errorf("a repeated name must say which file has it first: %q", got[3].Problem)
	}
}

func TestHomeWithoutATasksFolderHasNoSpecs(t *testing.T) {
	got, err := yamlfile.NewHome(fstest.MapFS{"README.md": {Data: []byte("x")}}).TaskSpecs(context.Background())
	if err != nil || len(got) != 0 {
		t.Fatalf("got %v, %v", got, err)
	}
}

type unreadable struct{ fstest.MapFS }

func (unreadable) ReadFile(string) ([]byte, error) { return nil, errors.New("disk on fire") }

func TestHomeListsAFileItCannotReadAsBroken(t *testing.T) {
	got, err := yamlfile.NewHome(unreadable{fstest.MapFS{"tasks/a.yaml": {Data: []byte("x")}}}).TaskSpecs(context.Background())
	if err != nil || len(got) != 1 || got[0].Valid() || !strings.Contains(got[0].Problem, "could not be read") {
		t.Fatalf("got %+v, %v", got, err)
	}
}

var _ fs.ReadFileFS = unreadable{}
