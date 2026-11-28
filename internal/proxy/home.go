package proxy

import (
	"net/url"
	"strconv"
	"strings"
)

// home is where the proxy's task reaches the portal's task-scope route, the publish route, which gets the task
// token in place of the placeholder the desk holds.
type home struct {
	host  string
	port  int
	route string
}

// homeOf is the portal's task-scope route of the task, or nil when the proxy holds no task token.
func homeOf(cfg Config) *home {
	u, err := url.Parse(cfg.Portal)
	if cfg.TaskToken == "" || err != nil || u.Hostname() == "" {
		return nil
	}
	port, err := strconv.Atoi(u.Port())
	if err != nil {
		port = 80
		if u.Scheme == "https" {
			port = 443
		}
	}
	route := strings.TrimRight(u.EscapedPath(), "/") + "/api/tasks/" + url.PathEscape(cfg.Task) + "/results"
	return &home{host: strings.ToLower(u.Hostname()), port: port, route: route}
}

// is is true when host and port are the portal's.
func (h *home) is(host string, port int) bool {
	return h != nil && h.host == host && h.port == port
}
