// Command fakegithub runs the fake GitHub that the tests use as a process, so a portal can be driven by hand against
// it. It serves the endpoints the GitHub adapter calls and keeps its comments, which GET /_fake/comments lists, and the
// installation tokens it was asked for, which GET /_fake/tokens lists. It
// makes a throwaway App key in a folder and never reaches the real API.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"

	"github.com/IniZio/moor/internal/adapter/github/githubtest"
)

func main() {
	keyDir := flag.String("key-dir", "", "folder to write the throwaway App key in")
	repo := flag.String("repo", "example-org/shop-web", "repository of the pull request the fake serves")
	number := flag.Int("number", 90, "number of the pull request")
	title := flag.String("title", "Add search to the shop", "title of the pull request")
	author := flag.String("author", "ana", "author of the pull request")
	sha := flag.String("sha", "5f0e2b19c4d7", "head commit of the pull request")
	flag.Parse()
	if *keyDir == "" {
		fmt.Fprintln(os.Stderr, "fakegithub: -key-dir is required")
		os.Exit(2)
	}
	key, err := githubtest.WriteKey(*keyDir)
	if err != nil {
		fmt.Fprintln(os.Stderr, "fakegithub:", err)
		os.Exit(1)
	}
	f := githubtest.New()
	defer f.Close()
	f.AddPullRequest(githubtest.PullRequest{Repo: *repo, Number: *number, Title: *title, Author: *author, SHA: *sha, Branch: "search"})
	fmt.Printf("url=%s\nkey=%s\n", f.URL(), key)
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	<-ctx.Done()
}
