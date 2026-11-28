package domain

import (
	"errors"
	"fmt"
	"regexp"
	"strings"
)

// DefaultRepoHost is the host of a repository written as a bare owner/name.
const DefaultRepoHost RepoHost = "github.com"

var ErrBadRepo = errors.New("bad repository")

var repoSegment = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)

// ParseRepo reads host/path or a bare owner/name. A bare owner/name gets DefaultRepoHost. Three or
// more segments need a host first, which has a dot or is localhost. Host and path are lower-cased,
// because forges treat them without case, and so equal repositories are equal values.
func ParseRepo(s string) (Repo, error) {
	parts := strings.Split(s, "/")
	for _, p := range parts {
		if p == "." || p == ".." || !repoSegment.MatchString(p) {
			return Repo{}, fmt.Errorf("%w: %q is not host/owner/name or owner/name", ErrBadRepo, s)
		}
	}
	host, path := DefaultRepoHost, parts
	if len(parts) != 2 {
		host, path = RepoHost(parts[0]), parts[1:]
		if len(path) < 2 || (!strings.Contains(parts[0], ".") && parts[0] != "localhost") {
			return Repo{}, fmt.Errorf("%w: %q needs a host with a dot first, or just owner/name", ErrBadRepo, s)
		}
	}
	return Repo{Host: RepoHost(strings.ToLower(string(host))), Path: RepoPath(strings.ToLower(strings.Join(path, "/")))}, nil
}
