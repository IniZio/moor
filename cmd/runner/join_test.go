package main

import (
	"crypto/ed25519"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type joinSeen struct {
	ID        string `json:"id"`
	Token     string `json:"token"`
	Name      string `json:"name"`
	PublicKey []byte `json:"public_key"`
}

func joinPortal(t *testing.T, status int, seen *[]joinSeen) string {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var j joinSeen
		if r.Method != http.MethodPost || r.URL.Path != "/runner/join" || json.NewDecoder(r.Body).Decode(&j) != nil {
			http.Error(w, "bad join", http.StatusBadRequest)
			return
		}
		*seen = append(*seen, j)
		w.WriteHeader(status)
		_, _ = w.Write([]byte("the join token is unknown, used or expired"))
	}))
	t.Cleanup(srv.Close)
	return srv.URL
}

func TestJoinMakesAKeyOnceAndSendsItsPublicHalf(t *testing.T) {
	var seen []joinSeen
	url := joinPortal(t, http.StatusOK, &seen)
	keyFile := filepath.Join(t.TempDir(), "runner.key")
	for range 2 {
		if err := run([]string{"join", "-enrol-only", "-portal", url, "-id", "m-1", "-token", "tok", "-key", keyFile, "-name", "host-a"}); err != nil {
			t.Fatal(err)
		}
	}
	info, err := os.Stat(keyFile)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("key file %v, %v, want mode 0600", info, err)
	}
	key, err := readKey(keyFile)
	if err != nil {
		t.Fatal(err)
	}
	pub := key.Public().(ed25519.PublicKey)
	if len(seen) != 2 || seen[0].ID != "m-1" || seen[0].Token != "tok" || seen[0].Name != "host-a" || !pub.Equal(ed25519.PublicKey(seen[1].PublicKey)) || !pub.Equal(ed25519.PublicKey(seen[0].PublicKey)) {
		t.Fatalf("joins seen %+v, want both with the key in %s", seen, keyFile)
	}
}

func TestARefusedJoinSaysWhy(t *testing.T) {
	var seen []joinSeen
	url := joinPortal(t, http.StatusForbidden, &seen)
	err := run([]string{"join", "-enrol-only", "-portal", url, "-id", "m-1", "-token", "tok", "-key", filepath.Join(t.TempDir(), "k"), "-name", "host-a"})
	if err == nil || !strings.Contains(err.Error(), "unknown, used or expired") {
		t.Fatalf("err = %v", err)
	}
}
