package link

import (
	"crypto/ed25519"
	"crypto/rand"
	"net/url"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestHostOfCanonicalisesWhatIsSigned(t *testing.T) {
	cases := map[string]string{
		"https://Portal.Example/runners/link": "portal.example",
		"wss://portal.example:443/x":          "portal.example",
		"https://portal.example:8443":         "portal.example:8443",
		"ws://127.0.0.1:80":                   "127.0.0.1",
		"ws://127.0.0.1:7300":                 "127.0.0.1:7300",
		"wss://[::1]:7300":                    "[::1]:7300",
	}
	for raw, want := range cases {
		u, err := url.Parse(raw)
		if err != nil {
			t.Fatal(err)
		}
		if got := HostOf(u); got != want {
			t.Errorf("%s: got %q, want %q", raw, got, want)
		}
	}
}

func TestProofBindsHostNonceRunnerAndProto(t *testing.T) {
	pub, key, _ := ed25519.GenerateKey(rand.Reader)
	nonce := make([]byte, 32)
	sig := proof{"portal.example", nonce, "r1", 1}.sign(key)
	other := make([]byte, 32)
	other[0] = 1
	cases := []struct {
		name   string
		host   string
		nonce  []byte
		runner domain.RunnerID
		proto  int
		known  bool
		ok     bool
	}{
		{"same", "portal.example", nonce, "r1", 1, true, true},
		{"rogue server relaying the challenge", "evil.example", nonce, "r1", 1, true, false},
		{"another connection's nonce", "portal.example", other, "r1", 1, true, false},
		{"another runner", "portal.example", nonce, "r2", 1, true, false},
		{"downgraded proto", "portal.example", nonce, "r1", 0, true, false},
		{"unknown runner, right signature", "portal.example", nonce, "r1", 1, false, false},
	}
	for _, c := range cases {
		claim := proof{c.host, c.nonce, c.runner, c.proto}
		if got := claim.verify(pub, c.known, sig); got != c.ok {
			t.Errorf("%s: got %v", c.name, got)
		}
	}
}

func TestProofFieldsCannotShift(t *testing.T) {
	a := proof{"ab", []byte("c"), "m", 1}.message()
	b := proof{"a", []byte("bc"), "m", 1}.message()
	if string(a) == string(b) {
		t.Fatal("moving a byte between fields must change the signed message")
	}
}
