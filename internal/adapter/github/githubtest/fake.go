// Package githubtest is a GitHub that speaks the few endpoints the adapter calls: the App's installation lookup, the
// installation token, a repository, a pull request, its comments, and a collaborator's permission. It checks nothing about who
// asks and keeps a record of every call, so a test can say what the adapter did. A test never reaches the real API.
package githubtest

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// PullRequest is what the fake serves for GET /repos/{owner}/{repo}/pulls/{number}.
type PullRequest struct {
	Repo   string
	Number int
	Title  string
	Body   string
	Author string
	SHA    string
	Branch string
	Draft  bool
}

// Comment is a comment on a pull request, as the fake keeps it.
type Comment struct {
	ID     int64
	Repo   string
	Number int
	Body   string
}

// Call is one request the fake served.
type Call struct{ Method, Path string }

// TokenRequest is what one request for an installation token asked for.
type TokenRequest struct {
	Installation string
	Repositories []string          `json:"repositories"`
	Permissions  map[string]string `json:"permissions"`
}

// Fake is the fake GitHub. Make it with New, and give the adapter URL().
type Fake struct {
	srv *httptest.Server

	mu          sync.Mutex
	prs         map[string]PullRequest
	comments    []Comment
	calls       []Call
	tokens      []TokenRequest
	permissions map[string]string
	branches    map[string]string
	failPosts   int
	nextID      int64
}

// New starts the fake. Close stops it.
func New() *Fake {
	f := &Fake{prs: map[string]PullRequest{}, permissions: map[string]string{}, branches: map[string]string{}, nextID: 1000}
	f.srv = httptest.NewServer(http.HandlerFunc(f.serve))
	return f
}

// NewForTest starts the fake and stops it when the test ends.
func NewForTest(t *testing.T) *Fake {
	t.Helper()
	f := New()
	t.Cleanup(f.Close)
	return f
}

func (f *Fake) Close()      { f.srv.Close() }
func (f *Fake) URL() string { return f.srv.URL }

// AddPullRequest makes a pull request that GET can read.
func (f *Fake) AddPullRequest(pr PullRequest) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.prs[key(pr.Repo, pr.Number)] = pr
}

// SetPermission sets what a user may do on a repository: admin, write, read or none.
func (f *Fake) SetPermission(repo, user, level string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.permissions[repo+"/"+user] = level
}

// FailPosts makes the next n requests that create or edit a comment fail with a server error.
func (f *Fake) FailPosts(n int) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.failPosts = n
}

// Comments lists the comments as they stand.
func (f *Fake) Comments() []Comment {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]Comment(nil), f.comments...)
}

// Calls lists the requests served, oldest first.
func (f *Fake) Calls() []Call {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]Call(nil), f.calls...)
}

// Count tells how many requests had this method and a path ending as given by suffix.
func (f *Fake) Count(method, pathPrefix string) int {
	n := 0
	for _, c := range f.Calls() {
		if c.Method == method && strings.HasPrefix(c.Path, pathPrefix) {
			n++
		}
	}
	return n
}

func key(repo string, n int) string { return strings.ToLower(repo) + "#" + strconv.Itoa(n) }

func (f *Fake) serve(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, Call{r.Method, r.URL.Path})
	parts := strings.Split(strings.Trim(r.URL.Path, "/"), "/")
	switch {
	case r.Method == http.MethodGet && r.URL.Path == "/_fake/comments":
		reply(w, http.StatusOK, f.comments)
	case r.Method == http.MethodGet && r.URL.Path == "/_fake/calls":
		reply(w, http.StatusOK, f.calls)
	case r.Method == http.MethodPost && len(parts) == 4 && parts[0] == "app" && parts[3] == "access_tokens":
		f.token(w, r, parts[2])
	case r.Method == http.MethodGet && len(parts) == 4 && parts[0] == "repos" && parts[3] == "installation":
		reply(w, http.StatusOK, map[string]any{"id": 4242})
	case len(parts) >= 3 && parts[0] == "repos":
		f.repoRoute(w, r, parts[1]+"/"+parts[2], parts[3:])
	default:
		http.NotFound(w, r)
	}
}

// TokenRequests lists the installation token requests, oldest first.
func (f *Fake) TokenRequests() []TokenRequest {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]TokenRequest(nil), f.tokens...)
}

func (f *Fake) token(w http.ResponseWriter, r *http.Request, installation string) {
	req := TokenRequest{Installation: installation}
	if r.ContentLength != 0 {
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			reply(w, http.StatusBadRequest, map[string]string{"message": err.Error()})
			return
		}
	}
	f.tokens = append(f.tokens, req)
	reply(w, http.StatusCreated, map[string]any{"token": "fake-token-" + installation, "expires_at": time.Now().Add(time.Hour).UTC().Format(time.RFC3339)})
}

// SetDefaultBranch names the repository's default branch, which is main until it is set.
func (f *Fake) SetDefaultBranch(repo, branch string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.branches[strings.ToLower(repo)] = branch
}

func (f *Fake) repoRoute(w http.ResponseWriter, r *http.Request, repo string, rest []string) {
	switch {
	case r.Method == http.MethodGet && len(rest) == 0:
		branch, ok := f.branches[strings.ToLower(repo)]
		if !ok {
			branch = "main"
		}
		reply(w, http.StatusOK, map[string]any{"full_name": repo, "default_branch": branch})
	case r.Method == http.MethodGet && len(rest) == 2 && rest[0] == "pulls":
		n, _ := strconv.Atoi(rest[1])
		f.pull(w, repo, n)
	case len(rest) == 3 && rest[0] == "issues" && rest[2] == "comments":
		n, _ := strconv.Atoi(rest[1])
		f.issueComments(w, r, repo, n)
	case r.Method == http.MethodPatch && len(rest) == 3 && rest[0] == "issues" && rest[1] == "comments":
		id, _ := strconv.ParseInt(rest[2], 10, 64)
		f.editComment(w, r, id)
	case r.Method == http.MethodGet && len(rest) == 3 && rest[0] == "collaborators" && rest[2] == "permission":
		level, ok := f.permissions[repo+"/"+rest[1]]
		if !ok {
			level = "none"
		}
		reply(w, http.StatusOK, map[string]any{"permission": level})
	default:
		http.NotFound(w, r)
	}
}

func (f *Fake) pull(w http.ResponseWriter, repo string, n int) {
	pr, ok := f.prs[key(repo, n)]
	if !ok {
		reply(w, http.StatusNotFound, map[string]string{"message": "Not Found"})
		return
	}
	reply(w, http.StatusOK, map[string]any{
		"number": pr.Number, "title": pr.Title, "body": pr.Body, "state": "open", "draft": pr.Draft,
		"html_url": fmt.Sprintf("https://github.com/%s/pull/%d", pr.Repo, pr.Number),
		"user":     map[string]any{"login": pr.Author},
		"head":     map[string]any{"sha": pr.SHA, "ref": pr.Branch, "repo": map[string]any{"full_name": pr.Repo}},
		"base":     map[string]any{"ref": "main", "repo": map[string]any{"full_name": pr.Repo, "html_url": "https://github.com/" + pr.Repo}},
	})
}

func (f *Fake) issueComments(w http.ResponseWriter, r *http.Request, repo string, n int) {
	if r.Method == http.MethodGet {
		var mine []map[string]any
		for _, c := range f.comments {
			if c.Repo == repo && c.Number == n {
				mine = append(mine, map[string]any{"id": c.ID, "body": c.Body})
			}
		}
		reply(w, http.StatusOK, mine)
		return
	}
	body, ok := f.commentBody(w, r)
	if !ok {
		return
	}
	f.nextID++
	f.comments = append(f.comments, Comment{ID: f.nextID, Repo: repo, Number: n, Body: body})
	reply(w, http.StatusCreated, map[string]any{"id": f.nextID, "body": body})
}

func (f *Fake) editComment(w http.ResponseWriter, r *http.Request, id int64) {
	body, ok := f.commentBody(w, r)
	if !ok {
		return
	}
	for i := range f.comments {
		if f.comments[i].ID == id {
			f.comments[i].Body = body
			reply(w, http.StatusOK, map[string]any{"id": id, "body": body})
			return
		}
	}
	reply(w, http.StatusNotFound, map[string]string{"message": "Not Found"})
}

// commentBody reads the body of a write, or fails the write while the fake is told to.
func (f *Fake) commentBody(w http.ResponseWriter, r *http.Request) (string, bool) {
	if f.failPosts > 0 {
		f.failPosts--
		reply(w, http.StatusInternalServerError, map[string]string{"message": "the fake GitHub is told to fail"})
		return "", false
	}
	var in struct {
		Body string `json:"body"`
	}
	if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
		reply(w, http.StatusBadRequest, map[string]string{"message": err.Error()})
		return "", false
	}
	return in.Body, true
}

func reply(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// NewKey makes the PEM of a throwaway RSA key. It is for the fake only.
func NewKey() ([]byte, error) {
	k, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return nil, fmt.Errorf("generate a key: %w", err)
	}
	return pem.EncodeToMemory(&pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(k)}), nil
}

// WriteKey makes a throwaway RSA key in dir and returns the path of its PEM file. It is for the fake only.
func WriteKey(dir string) (string, error) {
	pemBytes, err := NewKey()
	if err != nil {
		return "", err
	}
	path := filepath.Join(dir, "app-key.pem")
	if err := os.WriteFile(path, pemBytes, 0o600); err != nil {
		return "", fmt.Errorf("write the key: %w", err)
	}
	return path, nil
}
