package proxy_test

import (
	"bufio"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/IniZio/moor/internal/proxy"
	"github.com/IniZio/moor/internal/wire"
)

const (
	token = "proxy-token"
	pass  = "proxy-pass"
	// value is a synthetic connection value, never a real one.
	value = "synthetic-value-1234"
)

// fakePortal serves the three proxy routes from a policy the test changes.
type fakePortal struct {
	mu      sync.Mutex
	policy  wire.Policy
	asked   []string
	blocked []wire.BlockedReport
	ended   bool
}

func (f *fakePortal) set(p wire.Policy) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.policy = p
}

func (f *fakePortal) reports() []wire.BlockedReport {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]wire.BlockedReport(nil), f.blocked...)
}

func (f *fakePortal) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if r.Header.Get("Authorization") != "Bearer "+token || f.ended {
		http.Error(w, "no", http.StatusUnauthorized)
		return
	}
	switch r.URL.Path {
	case "/internal/policy/7":
		_ = json.NewEncoder(w).Encode(f.policy)
	case "/internal/connections/value":
		var in wire.ValueRequest
		_ = json.NewDecoder(r.Body).Decode(&in)
		f.asked = append(f.asked, in.Connection)
		_ = json.NewEncoder(w).Encode(wire.ValueReply{Value: value})
	case "/internal/blocked":
		var in wire.BlockedReport
		_ = json.NewDecoder(r.Body).Decode(&in)
		f.blocked = append(f.blocked, in)
		w.WriteHeader(http.StatusNoContent)
	default:
		http.NotFound(w, r)
	}
}

// echo answers each request with the headers it received.
func echo(w http.ResponseWriter, r *http.Request) {
	_ = json.NewEncoder(w).Encode(r.Header)
}

type rig struct {
	portal *fakePortal
	tlsUp  *httptest.Server
	plain  *httptest.Server
	client *http.Client
	proxy  string
	p      *proxy.Proxy
}

func newRig(t *testing.T, pol wire.Policy) *rig {
	t.Helper()
	g := &rig{portal: &fakePortal{policy: pol}}
	portal := httptest.NewServer(g.portal)
	g.tlsUp = httptest.NewTLSServer(http.HandlerFunc(echo))
	g.plain = httptest.NewServer(http.HandlerFunc(echo))
	ca, caPEM, err := proxy.NewCA(time.Now())
	if err != nil {
		t.Fatal(err)
	}
	upstream := x509.NewCertPool()
	upstream.AddCert(g.tlsUp.Certificate())
	g.p = proxy.New(proxy.Config{Portal: portal.URL, Task: "7", Token: token, Pass: pass, Poll: 20 * time.Millisecond, Upstream: &tls.Config{RootCAs: upstream}, CA: ca})
	srv := httptest.NewServer(g.p)
	ctx, cancel := context.WithCancel(context.Background())
	go g.p.Run(ctx)
	t.Cleanup(func() { cancel(); srv.Close(); portal.Close(); g.tlsUp.Close(); g.plain.Close() })
	trust := x509.NewCertPool()
	trust.AppendCertsFromPEM(caPEM)
	trust.AddCert(g.tlsUp.Certificate())
	u, _ := url.Parse(srv.URL)
	u.User = url.UserPassword("x", pass)
	g.proxy = u.String()
	g.client = &http.Client{Timeout: 5 * time.Second, Transport: &http.Transport{Proxy: http.ProxyURL(u), TLSClientConfig: &tls.Config{RootCAs: trust}}}
	waitFor(t, "the first policy", func() bool { return g.p.Refresh(context.Background()) == nil })
	return g
}

func waitFor(t *testing.T, what string, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !ok() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func hostPort(s *httptest.Server) string {
	return strings.TrimPrefix(strings.TrimPrefix(s.URL, "https://"), "http://")
}

// localhost names the plain upstream by name, so it and the TLS one have different hosts.
func localhost(s *httptest.Server) string {
	return "localhost:" + strings.Split(hostPort(s), ":")[1]
}

func do(t *testing.T, c *http.Client, method, u string, header http.Header) (*http.Response, string) {
	t.Helper()
	req, err := http.NewRequest(method, u, nil)
	if err != nil {
		t.Fatal(err)
	}
	for k, v := range header {
		req.Header[k] = v
	}
	resp, err := c.Do(req)
	if err != nil {
		t.Fatalf("%s %s: %v", method, u, err)
	}
	defer func() { _ = resp.Body.Close() }()
	b, _ := io.ReadAll(resp.Body)
	return resp, string(b)
}

func TestAnAllowedHTTPSRequestPassesWithTheConnectionHeaderSet(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(wire.Policy{Version: 1, Allow: []string{up + " GET"}, Connections: []wire.PolicyConnection{
		{Name: "Staging key", Hosts: []string{"127.0.0.1"}, Add: "header Authorization \"Bearer {value}\"", Set: true},
	}})
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, body := do(t, g.client, "GET", "https://"+up+"/x", http.Header{"Authorization": {"Bearer from-the-box"}})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("%s: %s", resp.Status, body)
	}
	var seen http.Header
	_ = json.Unmarshal([]byte(body), &seen)
	if seen.Get("Authorization") != "Bearer "+value || seen.Get("Proxy-Authorization") != "" {
		t.Fatalf("the upstream saw an Authorization of %d bytes, not the value, and Proxy-Authorization %q", len(seen.Get("Authorization")), seen.Get("Proxy-Authorization"))
	}
}

func TestAnUnlistedHostOrMethodIsRefusedWithItsReasonAndReported(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(wire.Policy{Version: 1, Allow: []string{up + " GET"}, Connections: []wire.PolicyConnection{
		{Name: "Staging key", Hosts: []string{"127.0.0.1"}, Add: "header X-Key \"{value}\"", Set: true},
	}})
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	cases := []struct{ method, url, reason, names string }{
		{"GET", "http://" + localhost(g.plain) + "/", "host_not_allowed", "GET " + localhost(g.plain)},
		{"POST", "https://" + up + "/", "method_not_allowed", "POST " + up},
	}
	for _, c := range cases {
		resp, body := do(t, g.client, c.method, c.url, nil)
		if resp.StatusCode != http.StatusForbidden || resp.Header.Get("X-Deny-Reason") != c.reason {
			t.Fatalf("%s %s: %s with reason %q", c.method, c.url, resp.Status, resp.Header.Get("X-Deny-Reason"))
		}
		if !strings.Contains(body, c.names) || !strings.Contains(body, c.reason) || !strings.Contains(body, "does not wait") {
			t.Errorf("the body does not name the request: %s", body)
		}
	}
	waitFor(t, "two reports", func() bool { return len(g.portal.reports()) == 2 })
	got := g.portal.reports()
	if got[0].Host != "localhost" || got[0].Method != "GET" || got[0].Reason != "host_not_allowed" || got[0].Task != "7" {
		t.Errorf("first report %+v", got[0])
	}
}

func TestATunnelToAnUnlistedHostIsRefusedWithItsReason(t *testing.T) {
	g := newRig(t, wire.Policy{Version: 1})
	resp := rawConnect(t, g.proxy, "127.0.0.1:1")
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusForbidden || resp.Header.Get("X-Deny-Reason") != "host_not_allowed" || !strings.Contains(string(body), "GET 127.0.0.1:1") {
		t.Fatalf("a tunnel to an unlisted host: %s, reason %q: %s", resp.Status, resp.Header.Get("X-Deny-Reason"), body)
	}
	waitFor(t, "the report", func() bool { return len(g.portal.reports()) == 1 })
}

// rawConnect asks the proxy for a tunnel and reads its answer, which a client library would not show.
func rawConnect(t *testing.T, proxyURL, target string) *http.Response {
	t.Helper()
	u, _ := url.Parse(proxyURL)
	c, err := net.Dial("tcp", u.Host)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.Close() })
	req := &http.Request{Method: http.MethodConnect, URL: &url.URL{Opaque: target}, Host: target, Header: http.Header{}}
	req.Header.Set("Proxy-Authorization", "Basic "+base64.StdEncoding.EncodeToString([]byte("x:"+pass)))
	if err := req.Write(c); err != nil {
		t.Fatal(err)
	}
	resp, err := http.ReadResponse(bufio.NewReader(c), req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func TestAnAllowedHostWithoutAConnectionIsTunnelledUntouched(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(wire.Policy{Version: 1, Allow: []string{up + " GET"}})
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, body := do(t, g.client, "POST", "https://"+up+"/", http.Header{"X-Mine": {"kept"}})
	var seen http.Header
	_ = json.Unmarshal([]byte(body), &seen)
	if resp.StatusCode != http.StatusOK || seen.Get("X-Mine") != "kept" || resp.TLS == nil || resp.TLS.PeerCertificates[0].Equal(g.tlsUp.Certificate()) == false {
		t.Fatalf("%s: the tunnel did not reach the upstream with its own certificate", resp.Status)
	}
}

func TestAnUpstreamTheProxyCannotVerifyAnswers502(t *testing.T) {
	g := newRig(t, wire.Policy{})
	stranger, _, err := proxy.NewCA(time.Now())
	if err != nil {
		t.Fatal(err)
	}
	other := httptest.NewUnstartedServer(http.HandlerFunc(echo))
	other.TLS = &tls.Config{Certificates: []tls.Certificate{*stranger}}
	other.StartTLS()
	defer other.Close()
	up := hostPort(other)
	g.portal.set(wire.Policy{Version: 1, Allow: []string{up + " GET"}, Connections: []wire.PolicyConnection{
		{Name: "Staging key", Hosts: []string{"127.0.0.1"}, Add: "placeholder STAGING", Set: true},
	}})
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, body := do(t, g.client, "GET", "https://"+up+"/", nil)
	if resp.StatusCode != http.StatusBadGateway || !strings.Contains(body, "could not reach") {
		t.Fatalf("%s: %s", resp.Status, body)
	}
}

func TestAPlaceholderIsReplacedInHeadersOnly(t *testing.T) {
	g := newRig(t, wire.Policy{})
	g.portal.set(wire.Policy{Version: 2, Allow: []string{localhost(g.plain) + " GET"}, Connections: []wire.PolicyConnection{
		{Name: "Search key", Hosts: []string{"localhost"}, Add: "placeholder SEARCH_KEY_PLACEHOLDER", Set: true},
	}})
	waitFor(t, "version 2", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, body := do(t, g.client, "GET", "http://"+localhost(g.plain)+"/?q=SEARCH_KEY_PLACEHOLDER", http.Header{"X-Key": {"key=SEARCH_KEY_PLACEHOLDER"}})
	var seen http.Header
	_ = json.Unmarshal([]byte(body), &seen)
	if resp.StatusCode != http.StatusOK || seen.Get("X-Key") != "key="+value {
		t.Fatalf("%s: the placeholder was not replaced", resp.Status)
	}
}

func TestASaveReachesTheProxyAtItsNextPoll(t *testing.T) {
	g := newRig(t, wire.Policy{Version: 1})
	u := "http://" + localhost(g.plain) + "/"
	if resp, _ := do(t, g.client, "GET", u, nil); resp.StatusCode != http.StatusForbidden {
		t.Fatalf("before the save: %s", resp.Status)
	}
	g.portal.set(wire.Policy{Version: 2, Allow: []string{localhost(g.plain) + " GET"}})
	waitFor(t, "the request passing", func() bool {
		resp, _ := do(t, g.client, "GET", u, nil)
		return resp.StatusCode == http.StatusOK
	})
}

func TestARefusedTokenDropsThePolicy(t *testing.T) {
	g := newRig(t, wire.Policy{Version: 1})
	u := "http://" + localhost(g.plain) + "/"
	g.portal.set(wire.Policy{Version: 2, Allow: []string{localhost(g.plain) + " GET"}})
	waitFor(t, "the request passing", func() bool { resp, _ := do(t, g.client, "GET", u, nil); return resp.StatusCode == http.StatusOK })
	g.portal.mu.Lock()
	g.portal.ended = true
	g.portal.mu.Unlock()
	waitFor(t, "the request refused once the task ended", func() bool {
		resp, _ := do(t, g.client, "GET", u, nil)
		return resp.StatusCode == http.StatusServiceUnavailable
	})
}

func TestAClientWithoutThePasswordIsRefused(t *testing.T) {
	g := newRig(t, wire.Policy{Version: 1, Allow: []string{"localhost:1 GET"}})
	u, _ := url.Parse(g.proxy)
	u.User = url.UserPassword("x", "wrong")
	c := &http.Client{Timeout: 5 * time.Second, Transport: &http.Transport{Proxy: http.ProxyURL(u)}}
	if resp, _ := do(t, c, "GET", "http://"+localhost(g.plain)+"/", nil); resp.StatusCode != http.StatusProxyAuthRequired {
		t.Fatalf("a plain request without the password: %s", resp.Status)
	}
	if _, err := c.Get("https://" + hostPort(g.tlsUp) + "/"); err == nil || !strings.Contains(err.Error(), "Proxy Authentication Required") {
		t.Fatalf("a tunnel without the password: %v", err)
	}
}
