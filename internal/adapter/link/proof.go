package link

import (
	"crypto/ed25519"
	"crypto/rand"
	"net"
	"net/url"
	"strconv"
	"strings"

	"github.com/IniZio/moor/internal/domain"
)

// proofLabel is the domain separator in the signed bytes, so a signature made for this protocol can
// never be a signature for anything else the key signs.
const proofLabel = "moor-link/1"

// refusedReason is what a server tells anyone who does not prove a known key. An unknown runner and a bad
// proof get the same answer, so the reply is not an oracle for which ids exist.
const refusedReason = "unauthorized"

// revokedReason is what a revoked runner is told once it has proved it holds the revoked key.
const revokedReason = "revoked"

// HostOf is the host both ends sign into a proof: lower-cased, with the port kept only when it is
// not the scheme's default. Both ends compute it from their own configuration, never from anything
// the peer sent.
func HostOf(u *url.URL) string {
	host, port := strings.ToLower(u.Hostname()), u.Port()
	if port == "" || (port == "443" && (u.Scheme == "https" || u.Scheme == "wss")) || (port == "80" && (u.Scheme == "http" || u.Scheme == "ws")) {
		return host
	}
	return net.JoinHostPort(host, port)
}

// proof is what a runner signs: the label, the server's public host, the per-connection nonce, the
// runner id and the protocol. Binding the host means a rogue server that relays a real server's
// challenge gets a proof for the wrong host. There is no timestamp: the nonce is fresh per
// connection, so a stale clock cannot matter and a captured proof is useless on the next one.
type proof struct {
	Host   string
	Nonce  []byte
	Runner domain.RunnerID
	Proto  int
}

// message length-prefixes each field, so no choice of fields can shift a boundary.
func (p proof) message() []byte {
	var b []byte
	for _, part := range []string{proofLabel, p.Host, string(p.Nonce), string(p.Runner), strconv.Itoa(p.Proto)} {
		b = strconv.AppendInt(b, int64(len(part)), 10)
		b = append(b, ':')
		b = append(b, part...)
		b = append(b, '\n')
	}
	return b
}

func (p proof) sign(key ed25519.PrivateKey) []byte { return ed25519.Sign(key, p.message()) }

// dummyKey stands in for an unknown runner's key, so verifying a proof for an id the store does not
// know costs about what a real verify costs. It narrows the timing difference, it does not prove
// constant time.
var dummyKey = func() ed25519.PublicKey {
	pub, _, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		panic(err)
	}
	return pub
}()

func (p proof) verify(key ed25519.PublicKey, known bool, sig []byte) bool {
	pub := key
	if !known {
		pub = dummyKey
	}
	ok := ed25519.Verify(pub, p.message(), sig)
	return known && ok
}
