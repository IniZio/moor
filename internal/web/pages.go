package web

import "net/http"

func (s *server) pageData(r *http.Request) page {
	return page{Person: personOf(r.Context()), Org: s.cfg.OrgName}
}

func (s *server) placeholder(title, nav string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		render(w, r, http.StatusOK, placeholderPage(s.pageData(r), title, nav))
	}
}

func (s *server) settingsPage(w http.ResponseWriter, r *http.Request) {
	cur, err := s.deps.Settings.Current(r.Context())
	if err != nil {
		serverError(w, r, err)
		return
	}
	render(w, r, http.StatusOK, settingsPage(s.pageData(r), cur))
}

func (s *server) me(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, map[string]string{"name": personOf(r.Context())})
}
