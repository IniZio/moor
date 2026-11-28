package proxy

import (
	"crypto/subtle"
	"crypto/tls"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/elazarl/goproxy"

	"github.com/IniZio/moor/internal/domain"
)

// Config wires a proxy. Portal is the portal's address, and Token the proxy's own token for Task. Pass is the
// password clients present in Proxy-Authorization. Upstream verifies upstream servers and takes the system
// roots when nil. CA signs the certificates the proxy shows its clients.
type Config struct {
	Portal   string
	Task     string
	Token    string
	Pass     string
	Poll     time.Duration
	Client   *http.Client
	Upstream *tls.Config
	CA       *tls.Certificate
	Log      *slog.Logger
}

// Proxy enforces the newest policy it read. Until it has read one it refuses everything.
type Proxy struct {
	cfg    Config
	gp     *goproxy.ProxyHttpServer
	policy atomic.Pointer[domain.Policy]
	mu     sync.Mutex
	values map[string]cached
	mitm   *goproxy.ConnectAction
}

// cached is a connection's value as read under one policy version. A newer version drops it, so a rotated
// or revoked value bites at the next request after the proxy reads the new version.
type cached struct {
	version int
	value   string
}

// authed marks a CONNECT that presented the password, for the requests read inside its tunnel.
type authed struct{}

func New(cfg Config) *Proxy {
	if cfg.Log == nil {
		cfg.Log = slog.New(slog.DiscardHandler)
	}
	if cfg.Client == nil {
		cfg.Client = &http.Client{Timeout: 10 * time.Second}
	}
	p := &Proxy{cfg: cfg, values: map[string]cached{}}
	gp := goproxy.NewProxyHttpServer()
	gp.Logger = slog.NewLogLogger(cfg.Log.Handler(), slog.LevelDebug)
	gp.Tr = &http.Transport{
		TLSClientConfig: cfg.Upstream, DialContext: (&net.Dialer{Timeout: 10 * time.Second}).DialContext,
		TLSHandshakeTimeout: 10 * time.Second, MaxIdleConnsPerHost: 4, IdleConnTimeout: time.Minute,
	}
	p.mitm = &goproxy.ConnectAction{Action: goproxy.ConnectMitm, TLSConfig: goproxy.TLSConfigFromCA(cfg.CA)}
	gp.OnRequest().HandleConnectFunc(p.connect)
	gp.OnRequest().DoFunc(p.handle)
	p.gp = gp
	return p
}

func (p *Proxy) ServeHTTP(w http.ResponseWriter, r *http.Request) { p.gp.ServeHTTP(w, r) }

// connect decides a tunnel by its host and port. A host with a connection is intercepted, so its requests
// get the value and their methods are checked. Any other allowed host is tunnelled as it is, so clients
// that do not trust the proxy's certificate, such as containers inside the Box, still reach it.
func (p *Proxy) connect(target string, ctx *goproxy.ProxyCtx) (*goproxy.ConnectAction, string) {
	r := ctx.Req
	if !p.authorized(r) {
		return answer(needPassword(r)), target
	}
	pol := p.policy.Load()
	if pol == nil {
		return answer(notReady(r)), target
	}
	host, port := splitTarget(target)
	if !pol.Tunnel(host, port) {
		go p.report(host, port, http.MethodGet, domain.DenyHost)
		return answer(deny(r, http.MethodGet, host, port, domain.DenyHost)), target
	}
	if c, ok := pol.ConnectionFor(host); ok && c.Secret == domain.SecretSet {
		ctx.UserData = authed{}
		return p.mitm, target
	}
	return goproxy.OkConnect, target
}

// answer replies to a CONNECT with resp and closes the connection.
func answer(resp *http.Response) *goproxy.ConnectAction {
	return &goproxy.ConnectAction{Action: goproxy.ConnectHijack, Hijack: func(_ *http.Request, client net.Conn, _ *goproxy.ProxyCtx) {
		resp.Close = true
		_ = resp.Write(client)
		_ = client.Close()
	}}
}

func splitTarget(target string) (string, int) {
	host, portText, err := net.SplitHostPort(target)
	port, perr := strconv.Atoi(portText)
	if err != nil || perr != nil {
		return strings.ToLower(target), 443
	}
	return strings.ToLower(host), port
}

func needPassword(r *http.Request) *http.Response {
	resp := goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusProxyAuthRequired, "the proxy needs its password\n")
	resp.Header.Set("Proxy-Authenticate", "Basic realm=\"task\"")
	return resp
}

func notReady(r *http.Request) *http.Response {
	return goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusServiceUnavailable, "the proxy has not read the task's policy yet, try again\n")
}

// roundTrip answers 502 with the reason when the upstream cannot be reached, where an intercepted tunnel
// would otherwise just close.
func (p *Proxy) roundTrip(r *http.Request, _ *goproxy.ProxyCtx) (*http.Response, error) {
	resp, err := p.gp.Tr.RoundTrip(r)
	if err != nil {
		return goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusBadGateway, "the proxy could not reach the upstream: "+err.Error()+"\n"), nil
	}
	return resp, nil
}

// authorized is true when the request presents the proxy's password as Basic credentials.
func (p *Proxy) authorized(r *http.Request) bool {
	h := http.Header{"Authorization": {r.Header.Get("Proxy-Authorization")}}
	_, pass, ok := (&http.Request{Header: h}).BasicAuth()
	return ok && subtle.ConstantTimeCompare([]byte(pass), []byte(p.cfg.Pass)) == 1
}

// handle decides one request, a plain one or one read inside an intercepted tunnel: it refuses what the policy does not
// allow, and adds the value of the connection that names the host.
func (p *Proxy) handle(r *http.Request, ctx *goproxy.ProxyCtx) (*http.Request, *http.Response) {
	if _, ok := ctx.UserData.(authed); !ok && !p.authorized(r) {
		resp := goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusProxyAuthRequired, "the proxy needs its password\n")
		resp.Header.Set("Proxy-Authenticate", "Basic realm=\"task\"")
		return r, resp
	}
	pol := p.policy.Load()
	if pol == nil {
		return r, goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusServiceUnavailable, "the proxy has not read the task's policy yet, try again\n")
	}
	ctx.RoundTripper = goproxy.RoundTripperFunc(p.roundTrip)
	host, port := target(r)
	method := domain.Method(r.Method)
	if reason, ok := pol.Check(host, port, method); !ok {
		go p.report(host, port, method, reason)
		return r, deny(r, r.Method, host, port, reason)
	}
	c, ok := pol.ConnectionFor(host)
	if !ok || c.Secret != domain.SecretSet {
		return r, nil
	}
	v, err := p.value(r.Context(), pol.Version, c.Name)
	if err != nil {
		p.cfg.Log.Warn("connection value", "connection", c.Name, "err", err)
		return r, goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusBadGateway, "the proxy could not read the value of connection "+strconv.Quote(c.Name)+"\n")
	}
	add(r, c.Add, v)
	return r, nil
}

// target is the request's host and port, the port from its scheme when the address names none.
func target(r *http.Request) (string, int) {
	port, err := strconv.Atoi(r.URL.Port())
	if err != nil {
		port = 80
		if r.URL.Scheme == "https" {
			port = 443
		}
	}
	return strings.ToLower(r.URL.Hostname()), port
}

// deny is the 403 a refused request gets. A refused tunnel names GET, the method a person would allow.
func deny(r *http.Request, method, host string, port int, reason domain.DenyReason) *http.Response {
	body := fmt.Sprintf("The organisation's policy refused %s %s:%d (%s).\n"+
		"The task does not wait for this. A person may allow it on the task's page, and a later attempt then passes.\n",
		method, host, port, reason)
	resp := goproxy.NewResponse(r, "text/plain; charset=utf-8", http.StatusForbidden, body)
	resp.Header.Set("X-Deny-Reason", string(reason))
	return resp
}

// add puts the value into the request as the connection's add rule says: a header set from its template,
// or the placeholder replaced in every header. Bodies are never touched.
func add(r *http.Request, rule domain.AddRule, value string) {
	switch a := rule.(type) {
	case domain.HeaderAdd:
		r.Header.Set(a.Name, strings.ReplaceAll(a.Template, "{value}", value))
	case domain.PlaceholderAdd:
		for name, vals := range r.Header {
			for i, v := range vals {
				r.Header[name][i] = strings.ReplaceAll(v, a.Token, value)
			}
		}
	}
}
