package githome

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"strings"
	"testing/fstest"

	"github.com/go-git/go-git/v5"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/object"
	"github.com/go-git/go-git/v5/plumbing/transport/http"
	"github.com/go-git/go-git/v5/storage/memory"
)

// Remote says what to fetch. Branch is empty for the repository's default branch. Token is the forge's
// installation token, sent as the password of a Basic credential.
type Remote struct {
	URL    string
	Branch string
	Token  string
}

// Snapshot is the home at one commit. Files holds the files under tasks/, with their paths from the root.
type Snapshot struct {
	Commit string
	Branch string
	Files  fs.FS
}

// tokenUser is the user name GitHub expects with an installation token.
const tokenUser = "x-access-token"

// specDir is the only folder the portal reads from the home.
const specDir = "tasks/"

// Fetch makes a shallow clone of one branch into memory and reads tasks/ from its tip. An error never names the
// token.
func Fetch(ctx context.Context, r Remote) (Snapshot, error) {
	opts := &git.CloneOptions{
		URL: r.URL, Depth: 1, SingleBranch: true, Tags: git.NoTags,
		Auth: &http.BasicAuth{Username: tokenUser, Password: r.Token},
	}
	if r.Branch != "" {
		opts.ReferenceName = plumbing.NewBranchReferenceName(r.Branch)
	}
	repo, err := git.CloneContext(ctx, memory.NewStorage(), nil, opts)
	if err != nil {
		return Snapshot{}, fmt.Errorf("fetch %s: %w", r.URL, redact(err, r.Token))
	}
	snap, err := read(repo)
	if err != nil {
		return Snapshot{}, fmt.Errorf("read %s: %w", r.URL, err)
	}
	return snap, nil
}

func redact(err error, token string) error {
	if token == "" {
		return err
	}
	return errors.New(strings.ReplaceAll(err.Error(), token, "***"))
}

func read(repo *git.Repository) (Snapshot, error) {
	head, err := repo.Head()
	if err != nil {
		return Snapshot{}, fmt.Errorf("head: %w", err)
	}
	commit, err := repo.CommitObject(head.Hash())
	if err != nil {
		return Snapshot{}, fmt.Errorf("commit %s: %w", head.Hash(), err)
	}
	tree, err := commit.Tree()
	if err != nil {
		return Snapshot{}, fmt.Errorf("tree of %s: %w", head.Hash(), err)
	}
	files := fstest.MapFS{}
	err = tree.Files().ForEach(func(f *object.File) error {
		if !strings.HasPrefix(f.Name, specDir) {
			return nil
		}
		text, err := f.Contents()
		if err != nil {
			return fmt.Errorf("%s: %w", f.Name, err)
		}
		files[f.Name] = &fstest.MapFile{Data: []byte(text)}
		return nil
	})
	if err != nil {
		return Snapshot{}, fmt.Errorf("files of %s: %w", head.Hash(), err)
	}
	return Snapshot{Commit: head.Hash().String(), Branch: head.Name().Short(), Files: files}, nil
}
