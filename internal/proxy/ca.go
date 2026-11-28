package proxy

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"math/big"
	"os"
	"path/filepath"
	"time"
)

// NewCA makes the certificate authority a proxy signs its clients' certificates with. The key never leaves
// the proxy's memory. The PEM of the certificate is what the clients trust.
func NewCA(now time.Time) (*tls.Certificate, []byte, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, nil, fmt.Errorf("ca key: %w", err)
	}
	serial, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 127))
	if err != nil {
		return nil, nil, fmt.Errorf("ca serial: %w", err)
	}
	tmpl := &x509.Certificate{
		SerialNumber: serial, Subject: pkix.Name{CommonName: "Task proxy"},
		NotBefore: now.Add(-31 * 24 * time.Hour), NotAfter: now.Add(400 * 24 * time.Hour),
		IsCA: true, BasicConstraintsValid: true, MaxPathLenZero: true,
		KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageDigitalSignature,
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		return nil, nil, fmt.Errorf("ca certificate: %w", err)
	}
	leaf, err := x509.ParseCertificate(der)
	if err != nil {
		return nil, nil, fmt.Errorf("ca certificate: %w", err)
	}
	return &tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key, Leaf: leaf}, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), nil
}

// WriteTrust writes the certificate to dir as ca.crt, and as bundle.pem after the system roots read from
// roots, each under its final name only once it is whole.
func WriteTrust(dir string, cert []byte, roots string) error {
	system, err := os.ReadFile(roots)
	if err != nil {
		return fmt.Errorf("system roots: %w", err)
	}
	bundle := append(append(system, '\n'), cert...)
	for name, data := range map[string][]byte{"ca.crt": cert, "bundle.pem": bundle} {
		tmp := filepath.Join(dir, "."+name)
		if err := os.WriteFile(tmp, data, 0o644); err != nil { //nolint:gosec // a certificate is public, and clients of another user read it
			return fmt.Errorf("write %s: %w", name, err)
		}
		if err := os.Rename(tmp, filepath.Join(dir, name)); err != nil {
			return fmt.Errorf("write %s: %w", name, err)
		}
	}
	return nil
}
