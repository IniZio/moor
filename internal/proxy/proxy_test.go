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
	expires time.Time
	calls   map[string]string
}

func (f *fakePortal) askedFor() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.asked...)
}

func rule(name string, host string, allow ...string) wire.PolicyRule {
	return wire.PolicyRule{Name: name, Hosts: []string{host}, Allow: allow}
}

func withCred(r wire.PolicyRule, cred string, in wire.Inject) wire.PolicyRule {
	r.Credential, r.Set, r.Inject = cred, true, &in
	return r
}

func policy(version int, rules ...wire.PolicyRule) wire.Policy {
	return wire.Policy{Version: version, Repo: wire.Repo{Host: "github.com", Path: "example-org/shop-web"}, Rules: rules}
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
	if strings.HasPrefix(r.URL.Path, "/api/") {
		f.calls[r.URL.Path] = r.Header.Get("Authorization")
		w.WriteHeader(http.StatusCreated)
		return
	}
	if r.Header.Get("Authorization") != "Bearer "+token || f.ended {
		http.Error(w, "no", http.StatusUnauthorized)
		return
	}
	switch r.URL.Path {
	case "/internal/policy/7":
		_ = json.NewEncoder(w).Encode(f.policy)
	case "/internal/credentials/value":
		var in wire.ValueRequest
		_ = json.NewDecoder(r.Body).Decode(&in)
		f.asked = append(f.asked, in.Credential)
		_ = json.NewEncoder(w).Encode(wire.ValueReply{Value: value, Expires: f.expires})
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
	home   *httptest.Server
}

// newRig starts the proxy under pol. Each opt changes the proxy's config before it starts.
func newRig(t *testing.T, pol wire.Policy, opts ...func(*proxy.Config)) *rig {
	t.Helper()
	g := &rig{portal: &fakePortal{policy: pol, calls: map[string]string{}}}
	portal := httptest.NewServer(g.portal)
	g.home = portal
	g.tlsUp = httptest.NewTLSServer(http.HandlerFunc(echo))
	g.plain = httptest.NewServer(http.HandlerFunc(echo))
	ca, caPEM, err := proxy.NewCA(time.Now())
	if err != nil {
		t.Fatal(err)
	}
	upstream := x509.NewCertPool()
	upstream.AddCert(g.tlsUp.Certificate())
	cfg := proxy.Config{Portal: portal.URL, Task: "7", Token: token, Pass: pass, Poll: 20 * time.Millisecond, Upstream: &tls.Config{RootCAs: upstream}, CA: ca}
	for _, o := range opts {
		o(&cfg)
	}
	g.p = proxy.New(cfg)
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

func TestAnAllowedHTTPSRequestPassesWithTheCredentialHeaderSet(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(policy(1, withCred(rule("staging", up, "GET /**"), "staging", wire.Inject{Header: "Authorization", Form: "Bearer {secret}"})))
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

func TestABasicUserInjectSetsTheValueAsThePassword(t *testing.T) {
	g := newRig(t, wire.Policy{})
	host := localhost(g.plain)
	g.portal.set(policy(1, withCred(rule("git", host, "GET|POST /{repo}.git/**"), "github", wire.Inject{BasicUser: "x-access-token"})))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, body := do(t, g.client, "GET", "http://"+host+"/example-org/shop-web.git/info/refs?service=git-upload-pack", http.Header{"Authorization": {"Basic eDptb29yLXBsYWNlaG9sZGVy"}})
	var seen http.Header
	_ = json.Unmarshal([]byte(body), &seen)
	want := "Basic " + base64.StdEncoding.EncodeToString([]byte("x-access-token:"+value))
	if resp.StatusCode != http.StatusOK || seen.Get("Authorization") != want {
		t.Fatalf("%s: the upstream did not get the value as the password", resp.Status)
	}
	if resp, _ := do(t, g.client, "GET", "http://"+host+"/example-org/other.git/info/refs", nil); resp.StatusCode != http.StatusForbidden {
		t.Fatalf("another repository: %s", resp.Status)
	}
}

func TestAnUnlistedHostOrRequestIsRefusedWithItsReasonAndReported(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(policy(1, withCred(rule("staging", up, "GET /**"), "staging", wire.Inject{Header: "X-Key", Form: "{secret}"})))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	cases := []struct{ method, url, reason, names string }{
		{"GET", "http://" + localhost(g.plain) + "/a?token=x", "host_not_allowed", "GET " + localhost(g.plain)},
		{"POST", "https://" + up + "/orders/example-org/shop-web/7", "request_not_allowed", "POST " + up},
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
	first := wire.BlockedReport{Task: "7", Host: "localhost", Port: got[0].Port, Method: "GET", Path: "/a", Entry: "GET /a", Reason: "host_not_allowed"}
	second := wire.BlockedReport{
		Task: "7", Host: "127.0.0.1", Port: got[1].Port, Method: "POST", Path: "/orders/example-org/shop-web/7", Rule: "staging",
		Entry: "POST /orders/{repo}/7", Reason: "request_not_allowed",
	}
	if got[0] != first || got[1] != second {
		t.Errorf("reports %+v", got)
	}
}

func TestAnAskEntryHoldsARequestUntilItMovesToAllow(t *testing.T) {
	g := newRig(t, wire.Policy{})
	host := localhost(g.plain)
	held := rule("shop", host, "GET /**")
	held.Ask = []string{"DELETE /items/*"}
	g.portal.set(policy(1, held))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, _ := do(t, g.client, "DELETE", "http://"+host+"/items/3", nil)
	if resp.StatusCode != http.StatusForbidden || resp.Header.Get("X-Deny-Reason") != "needs_approval" {
		t.Fatalf("an ask entry: %s %q", resp.Status, resp.Header.Get("X-Deny-Reason"))
	}
	waitFor(t, "the report", func() bool { return len(g.portal.reports()) == 1 })
	if r := g.portal.reports()[0]; r.Rule != "shop" || r.Entry != "DELETE /items/*" || r.Path != "/items/3" {
		t.Fatalf("report %+v", r)
	}
	g.portal.set(policy(2, rule("shop", host, "GET /**", "DELETE /items/*")))
	waitFor(t, "the request passing", func() bool {
		resp, _ := do(t, g.client, "DELETE", "http://"+host+"/items/3", nil)
		return resp.StatusCode == http.StatusOK
	})
}

func TestAMintedTokenIsReadAgainBeforeItExpires(t *testing.T) {
	g := newRig(t, wire.Policy{})
	host := localhost(g.plain)
	g.portal.mu.Lock()
	g.portal.expires = time.Now().Add(30 * time.Second)
	g.portal.mu.Unlock()
	g.portal.set(policy(1, withCred(rule("gh", host, "GET /**"), "github", wire.Inject{Header: "Authorization", Form: "token {secret}"})))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	for range 2 {
		if resp, _ := do(t, g.client, "GET", "http://"+host+"/", nil); resp.StatusCode != http.StatusOK {
			t.Fatalf("%s", resp.Status)
		}
	}
	if got := g.portal.askedFor(); len(got) != 2 {
		t.Fatalf("a token a minute from expiry was read %d times for two requests, want 2", len(got))
	}
}

func TestATunnelToAnUnlistedHostIsRefusedWithItsReason(t *testing.T) {
	g := newRig(t, policy(1))
	resp := rawConnect(t, g.proxy, "127.0.0.1:1")
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusForbidden || resp.Header.Get("X-Deny-Reason") != "host_not_allowed" || !strings.Contains(string(body), "CONNECT 127.0.0.1:1") {
		t.Fatalf("a tunnel to an unlisted host: %s, reason %q: %s", resp.Status, resp.Header.Get("X-Deny-Reason"), body)
	}
	waitFor(t, "the report", func() bool { return len(g.portal.reports()) == 1 })
	if r := g.portal.reports()[0]; r.Method != "CONNECT" || r.Entry != "*" || r.Path != "" {
		t.Fatalf("report %+v", r)
	}
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

func TestAnAnyRuleWithoutACredentialIsTunnelledUntouched(t *testing.T) {
	g := newRig(t, wire.Policy{})
	up := hostPort(g.tlsUp)
	g.portal.set(policy(1, rule("db", up, "*")))
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
	g.portal.set(policy(1, rule("other", up, "GET /**")))
	waitFor(t, "version 1", func() bool { return g.p.Refresh(context.Background()) == nil })
	resp, body := do(t, g.client, "GET", "https://"+up+"/", nil)
	if resp.StatusCode != http.StatusBadGateway || !strings.Contains(body, "could not reach") {
		t.Fatalf("%s: %s", resp.Status, body)
	}
}

func TestASaveReachesTheProxyAtItsNextPoll(t *testing.T) {
	g := newRig(t, policy(1))
	u := "http://" + localhost(g.plain) + "/"
	if resp, _ := do(t, g.client, "GET", u, nil); resp.StatusCode != http.StatusForbidden {
		t.Fatalf("before the save: %s", resp.Status)
	}
	g.portal.set(policy(2, rule("plain", localhost(g.plain), "GET /")))
	waitFor(t, "the request passing", func() bool {
		resp, _ := do(t, g.client, "GET", u, nil)
		return resp.StatusCode == http.StatusOK
	})
}

func TestARefusedTokenDropsThePolicy(t *testing.T) {
	g := newRig(t, policy(1))
	u := "http://" + localhost(g.plain) + "/"
	g.portal.set(policy(2, rule("plain", localhost(g.plain), "*")))
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
	g := newRig(t, policy(1, rule("one", "localhost:1", "*")))
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
