// Command gitserver is the git server of the push test: git http-backend behind net/http/cgi on port 80, serving
// the bare repositories under /srv/git. A request without the Basic credential EXPECT_AUTH names gets 401, so a
// push that passes proves the proxy added it.
package main

import (
	"io"
	"log"
	"net/http"
	"net/http/cgi"
	"os"
	"time"
)

// spool writes a chunked body to a file and gives the request its length, since CGI takes no chunked body. It
// gives the file's name for the caller to remove.
func spool(r *http.Request) (string, error) {
	f, err := os.CreateTemp("", "push-")
	if err != nil {
		return "", err
	}
	n, err := io.Copy(f, r.Body)
	if err == nil {
		_, err = f.Seek(0, io.SeekStart)
	}
	r.Body, r.ContentLength, r.TransferEncoding = f, n, nil
	return f.Name(), err
}

func main() {
	want := os.Getenv("EXPECT_AUTH")
	backend := &cgi.Handler{
		Path: "/usr/bin/git", Args: []string{"http-backend"},
		Env: []string{"GIT_PROJECT_ROOT=/srv/git", "GIT_HTTP_EXPORT_ALL=1", "REMOTE_USER=task"},
	}
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		log.Printf("request method=%s path=%s", r.Method, r.URL.Path)
		if r.Header.Get("Authorization") != want {
			w.Header().Set("WWW-Authenticate", `Basic realm="git"`)
			http.Error(w, "no credential", http.StatusUnauthorized)
			return
		}
		if r.ContentLength < 0 {
			spooled, err := spool(r)
			if err != nil {
				http.Error(w, err.Error(), http.StatusBadRequest)
				return
			}
			defer func() { _ = os.Remove(spooled) }()
		}
		backend.ServeHTTP(w, r)
	})
	log.Fatal((&http.Server{Addr: ":80", Handler: h, ReadHeaderTimeout: 5 * time.Second}).ListenAndServe())
}
