// Package githometest is a git server for tests: one repository kept in memory, served over smart HTTP behind a
// token. It records who asked, so a test can say what the portal sent. A test never reaches a real forge.
package githometest

import (
	"compress/gzip"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/filemode"
	"github.com/go-git/go-git/v5/plumbing/format/pktline"
	"github.com/go-git/go-git/v5/plumbing/object"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp/capability"
	"github.com/go-git/go-git/v5/plumbing/storer"
	"github.com/go-git/go-git/v5/plumbing/transport"
	"github.com/go-git/go-git/v5/plumbing/transport/server"
	"github.com/go-git/go-git/v5/storage/memory"
)

// Request is one request the server answered, with the credential it came with.
type Request struct {
	Path, User, Password string
}

// Server is a git server for one repository. Make it with New.
type Server struct {
	srv  *httptest.Server
	path string
	git  transport.Transport
	st   *memory.Storage

	mu       sync.Mutex
	token    string
	fail     int
	requests []Request
}

// New starts a server for the repository owner/name that asks for token as the password of a Basic credential.
func New(t *testing.T, repo, token string) *Server {
	t.Helper()
	s := &Server{path: "/" + repo + ".git", st: memory.NewStorage(), token: token}
	s.git = server.NewServer(loader{s})
	s.srv = httptest.NewServer(http.HandlerFunc(s.serve))
	t.Cleanup(s.srv.Close)
	return s
}

// URL is the address of the server, with no path. A repository is at URL()/owner/name.git.
func (s *Server) URL() string { return s.srv.URL }

// Fail makes the next n requests answer 500.
func (s *Server) Fail(n int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.fail = n
}

// Requests lists what the server answered, oldest first.
func (s *Server) Requests() []Request {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Request(nil), s.requests...)
}

// Commit makes a commit on branch whose tree holds exactly files, and moves the branch to it. The first branch
// committed is the default. It returns the commit's hash.
func (s *Server) Commit(branch string, files map[string]string) (string, error) {
	tree, err := s.writeTree(files)
	if err != nil {
		return "", err
	}
	ref := plumbing.NewBranchReferenceName(branch)
	c := &object.Commit{
		Author:   object.Signature{Name: "example", Email: "example@example.test", When: time.Now()},
		TreeHash: tree, Message: "commit on " + branch,
	}
	c.Committer = c.Author
	if old, err := s.st.Reference(ref); err == nil {
		c.ParentHashes = []plumbing.Hash{old.Hash()}
	}
	obj := s.st.NewEncodedObject()
	if err := c.Encode(obj); err != nil {
		return "", fmt.Errorf("encode commit: %w", err)
	}
	hash, err := s.st.SetEncodedObject(obj)
	if err != nil {
		return "", fmt.Errorf("store commit: %w", err)
	}
	if err := s.st.SetReference(plumbing.NewHashReference(ref, hash)); err != nil {
		return "", fmt.Errorf("move %s: %w", branch, err)
	}
	if _, err := s.st.Reference(plumbing.HEAD); err != nil {
		if err := s.st.SetReference(plumbing.NewSymbolicReference(plumbing.HEAD, ref)); err != nil {
			return "", fmt.Errorf("set HEAD: %w", err)
		}
	}
	return hash.String(), nil
}

// writeTree stores the files as blobs under nested trees and returns the root tree.
func (s *Server) writeTree(files map[string]string) (plumbing.Hash, error) {
	entries, err := s.entries(split(files))
	if err != nil {
		return plumbing.ZeroHash, err
	}
	sort.Slice(entries, func(i, j int) bool { return entryKey(entries[i]) < entryKey(entries[j]) })
	obj := s.st.NewEncodedObject()
	if err := (&object.Tree{Entries: entries}).Encode(obj); err != nil {
		return plumbing.ZeroHash, fmt.Errorf("encode tree: %w", err)
	}
	hash, err := s.st.SetEncodedObject(obj)
	if err != nil {
		return plumbing.ZeroHash, fmt.Errorf("store tree: %w", err)
	}
	return hash, nil
}

// split separates the files at the top of a tree from those in each folder, whose paths it shortens.
func split(files map[string]string) (here map[string]string, dirs map[string]map[string]string) {
	here, dirs = map[string]string{}, map[string]map[string]string{}
	for name, text := range files {
		dir, rest, nested := strings.Cut(name, "/")
		if !nested {
			here[name] = text
			continue
		}
		if dirs[dir] == nil {
			dirs[dir] = map[string]string{}
		}
		dirs[dir][rest] = text
	}
	return here, dirs
}

func (s *Server) entries(here map[string]string, dirs map[string]map[string]string) ([]object.TreeEntry, error) {
	entries := []object.TreeEntry{}
	for name, text := range here {
		hash, err := s.blob(text)
		if err != nil {
			return nil, err
		}
		entries = append(entries, object.TreeEntry{Name: name, Mode: filemode.Regular, Hash: hash})
	}
	for dir, sub := range dirs {
		hash, err := s.writeTree(sub)
		if err != nil {
			return nil, err
		}
		entries = append(entries, object.TreeEntry{Name: dir, Mode: filemode.Dir, Hash: hash})
	}
	return entries, nil
}

// entryKey orders tree entries as git does, with a folder sorted as if its name ended in a slash.
func entryKey(e object.TreeEntry) string {
	if e.Mode == filemode.Dir {
		return e.Name + "/"
	}
	return e.Name
}

func (s *Server) blob(text string) (plumbing.Hash, error) {
	obj := s.st.NewEncodedObject()
	obj.SetType(plumbing.BlobObject)
	w, err := obj.Writer()
	if err != nil {
		return plumbing.ZeroHash, fmt.Errorf("blob: %w", err)
	}
	if _, err := w.Write([]byte(text)); err != nil {
		return plumbing.ZeroHash, fmt.Errorf("blob: %w", err)
	}
	if err := w.Close(); err != nil {
		return plumbing.ZeroHash, fmt.Errorf("blob: %w", err)
	}
	hash, err := s.st.SetEncodedObject(obj)
	if err != nil {
		return plumbing.ZeroHash, fmt.Errorf("store blob: %w", err)
	}
	return hash, nil
}

// loader gives go-git's server the one repository.
type loader struct{ s *Server }

func (l loader) Load(ep *transport.Endpoint) (storer.Storer, error) {
	if ep.Path != l.s.path {
		return nil, transport.ErrRepositoryNotFound
	}
	return l.s.st, nil
}

func (s *Server) allowed(w http.ResponseWriter, r *http.Request) bool {
	user, password, _ := r.BasicAuth()
	s.mu.Lock()
	s.requests = append(s.requests, Request{Path: r.URL.Path, User: user, Password: password})
	failing := s.fail > 0
	if failing {
		s.fail--
	}
	s.mu.Unlock()
	switch {
	case failing:
		http.Error(w, "the test server is told to fail", http.StatusInternalServerError)
	case password != s.token:
		w.Header().Set("WWW-Authenticate", `Basic realm="git"`)
		http.Error(w, "no credential", http.StatusUnauthorized)
	default:
		return true
	}
	return false
}

func (s *Server) serve(w http.ResponseWriter, r *http.Request) {
	if !s.allowed(w, r) {
		return
	}
	ep, err := transport.NewEndpoint("http://example.test" + s.path)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	sess, err := s.git.NewUploadPackSession(ep, nil)
	if err != nil {
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}
	defer func() { _ = sess.Close() }()
	switch {
	case r.Method == http.MethodGet && r.URL.Path == s.path+"/info/refs" && r.URL.Query().Get("service") == "git-upload-pack":
		s.advertise(w, r, sess)
	case r.Method == http.MethodPost && r.URL.Path == s.path+"/git-upload-pack":
		s.upload(w, r, sess)
	default:
		http.NotFound(w, r)
	}
}

func (s *Server) advertise(w http.ResponseWriter, r *http.Request, sess transport.UploadPackSession) {
	refs, err := sess.AdvertisedReferencesContext(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	// The server ignores the depth it is asked for and sends the whole history, which the client accepts.
	_ = refs.Capabilities.Set(capability.Shallow)
	refs.Prefix = [][]byte{[]byte("# service=git-upload-pack"), pktline.Flush}
	w.Header().Set("Content-Type", "application/x-git-upload-pack-advertisement")
	if err := refs.Encode(w); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
	}
}

func (s *Server) upload(w http.ResponseWriter, r *http.Request, sess transport.UploadPackSession) {
	body := r.Body
	if r.Header.Get("Content-Encoding") == "gzip" {
		gz, err := gzip.NewReader(r.Body)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		defer func() { _ = gz.Close() }()
		body = gz
	}
	req := packp.NewUploadPackRequest()
	if err := req.Decode(body); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	req.Capabilities.Delete(capability.Shallow)
	req.Depth = packp.DepthCommits(0)
	resp, err := sess.UploadPack(r.Context(), req)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer func() { _ = resp.Close() }()
	w.Header().Set("Content-Type", "application/x-git-upload-pack-result")
	_, _ = w.Write(pktline.FlushPkt)
	if err := resp.Encode(w); err != nil && !errors.Is(err, r.Context().Err()) {
		http.Error(w, err.Error(), http.StatusInternalServerError)
	}
}
