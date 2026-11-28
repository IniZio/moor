package web

import (
	"context"
	"net/http"
	"strings"
	"unicode"
)

type personKey struct{}

const maxPersonLen = 200

func personOf(ctx context.Context) string {
	name, _ := ctx.Value(personKey{}).(string)
	return name
}

// person reads the signed-in person from the configured header. Without one the request is refused, since
// the proxy in front would have sent it.
func (s *server) person(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		name := strings.TrimSpace(r.Header.Get(s.cfg.UserHeader))
		if name == "" || len(name) > maxPersonLen || strings.ContainsFunc(name, unicode.IsControl) {
			http.Error(w, "sign in first", http.StatusUnauthorized)
			return
		}
		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), personKey{}, name)))
	})
}
