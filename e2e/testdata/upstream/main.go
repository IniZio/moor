// Command upstream is the fake upstream of the access test: it answers ok on port 80, and on port 443 with a
// certificate no one trusts. It logs each request's host, the X-Api-Key and Authorization it carried, its method and
// its path, for the test to count.
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"log"
	"math/big"
	"net/http"
	"time"
)

func main() {
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		log.Printf("request host=%s key=%s auth=%s method=%s path=%s", r.Host, r.Header.Get("X-Api-Key"), r.Header.Get("Authorization"), r.Method, r.URL.Path)
		_, _ = w.Write([]byte("ok\n"))
	})
	go func() { log.Fatal((&http.Server{Addr: ":80", Handler: h, ReadHeaderTimeout: 5 * time.Second}).ListenAndServe()) }()
	cert, err := selfSigned()
	if err != nil {
		log.Fatal(err)
	}
	srv := &http.Server{Addr: ":443", Handler: h, ReadHeaderTimeout: 5 * time.Second, TLSConfig: &tls.Config{Certificates: []tls.Certificate{cert}}}
	log.Fatal(srv.ListenAndServeTLS("", ""))
}

func selfSigned() (tls.Certificate, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return tls.Certificate{}, err
	}
	tmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "upstream"}, DNSNames: []string{"secure-api.test"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour)}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		return tls.Certificate{}, err
	}
	return tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key}, nil
}
