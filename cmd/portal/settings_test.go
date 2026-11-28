package main

import (
	"context"
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"testing"

	"github.com/IniZio/moor/internal/adapter/secretbox"
	"github.com/IniZio/moor/internal/adapter/store"
	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/settings"
)

const goodDoc = `# example-org settings
repositories:
  - example-org/shop-web
allow:
  - api.example.com:443 GET
connections:
  - name: shop staging API
    hosts: [api.example.com]
    add: header Authorization "Bearer {value}"
    value: not set
`

const typedSecret = "hunter2-token-9f3a"

type saveBody struct {
	Base int    `json:"base"`
	Text string `json:"text"`
	Note string `json:"note,omitempty"`
}

func put(person string, base int, text string) req {
	b, _ := json.Marshal(saveBody{Base: base, Text: text})
	return req{method: http.MethodPut, path: "/api/settings", person: person, json: true, body: string(b)}
}

func hx(r req) req {
	r.headers = map[string]string{"HX-Request": "true"}
	return r
}

func putForm(person string, base string, text string) req {
	return hx(req{method: http.MethodPut, path: "/api/settings", person: person, form: url.Values{"base": {base}, "text": {text}}})
}

type current struct {
	Version int    `json:"version"`
	Text    string `json:"text"`
}

func (a *app) current() current {
	a.t.Helper()
	r := a.do(get("Mia", "/api/settings"))
	var c current
	if err := json.Unmarshal([]byte(r.body), &c); err != nil || r.code != http.StatusOK {
		a.t.Fatalf("current: %d %q %v", r.code, r.body, err)
	}
	return c
}

func TestBeforeAnySaveTheEditorHoldsTheInitialTextAndItSavesAsIs(t *testing.T) {
	a := newApp(t)
	if c := a.current(); c.Version != 0 || c.Text != settings.InitialText {
		t.Fatalf("current = %+v", c)
	}
	a.do(put("Mia", 0, settings.InitialText)).has(t, http.StatusOK, `"version":1`)
}

func TestASavedDocumentIsReadBackWithItsVersionAndHistory(t *testing.T) {
	a := newApp(t)
	a.do(put("Mia", 0, goodDoc)).has(t, http.StatusOK, `"version":1`, `"unchanged":false`)
	c := a.current()
	if c.Version != 1 || !strings.Contains(c.Text, "# example-org settings") || !strings.Contains(c.Text, "value: not set") {
		t.Fatalf("current = %+v", c)
	}
	a.do(put("Ken", 1, strings.Replace(c.Text, "- api.example.com:443 GET", "- api.example.com:443 GET POST", 1))).has(t, http.StatusOK, `"version":2`)
	var versions []struct {
		Version int    `json:"version"`
		SavedBy string `json:"saved_by"`
		Diff    []struct{ Kind, Text string }
	}
	r := a.do(get("Mia", "/api/settings/versions"))
	r.has(t, http.StatusOK)
	if err := json.Unmarshal([]byte(r.body), &versions); err != nil || len(versions) != 2 {
		t.Fatalf("versions = %q, %v", r.body, err)
	}
	if versions[0].Version != 2 || versions[0].SavedBy != "Ken" || versions[1].SavedBy != "Mia" {
		t.Errorf("versions = %+v", versions)
	}
	added, removed := diffLines(versions[0].Diff)
	if len(added) != 1 || !strings.Contains(added[0], "GET POST") || len(removed) != 1 || strings.Contains(removed[0], "POST") {
		t.Errorf("diff of v2: +%q -%q", added, removed)
	}
}

func diffLines(diff []struct{ Kind, Text string }) (added, removed []string) {
	for _, l := range diff {
		switch l.Kind {
		case "added":
			added = append(added, l.Text)
		case "removed":
			removed = append(removed, l.Text)
		}
	}
	return added, removed
}

func TestSavingTheTextYouReadBackChangesNothing(t *testing.T) {
	a := newApp(t)
	a.do(put("Mia", 0, goodDoc)).has(t, http.StatusOK)
	c := a.current()
	a.do(put("Mia", 1, c.Text)).has(t, http.StatusOK, `"version":1`, `"unchanged":true`)
	a.do(putForm("Mia", "1", c.Text)).has(t, http.StatusOK, "No changes to save.")
	if got := a.current(); got.Version != 1 {
		t.Errorf("an unchanged save made v%d", got.Version)
	}
}

func TestASaveFromAnOldVersionIsRefused(t *testing.T) {
	a := newApp(t)
	a.do(put("Mia", 0, goodDoc)).has(t, http.StatusOK)
	a.do(put("Ken", 0, goodDoc+"# more\n")).has(t, http.StatusConflict, `"newest":1`)
	a.do(putForm("Ken", "0", goodDoc+"# more\n")).has(t, http.StatusConflict, "Not saved. Someone saved v1 while you were editing.")
	if got := a.current(); got.Version != 1 || strings.Contains(got.Text, "more") {
		t.Errorf("a stale save changed the document: %+v", got)
	}
}

func TestAnInvalidDocumentGetsItsLineErrorsAndSavesNothing(t *testing.T) {
	a := newApp(t)
	bad := strings.Replace(goodDoc, "- api.example.com:443 GET", "- api.example.com GET", 1)
	r := a.do(put("Mia", 0, bad))
	r.has(t, http.StatusUnprocessableEntity, `"line":5`, "allow rule")
	r.lacks(t, "bad settings:")
	h := a.do(putForm("Mia", "0", bad))
	h.has(t, http.StatusUnprocessableEntity, "Not saved. Fix the lines marked in red.", `data-lines="5"`, "<b>Line 5</b>", `hx-swap-oob`)
	if h.header.Get("HX-Reswap") != "none" {
		t.Errorf("a refused save must leave the editor as it is, HX-Reswap = %q", h.header.Get("HX-Reswap"))
	}
	broken := a.do(put("Mia", 0, "repositories: [\n"))
	broken.has(t, http.StatusUnprocessableEntity, `"line"`)
	if got := a.current(); got.Version != 0 {
		t.Errorf("a refused save made v%d", got.Version)
	}
}

func TestTheLiveCheckListsTheLinesWithoutSaving(t *testing.T) {
	a := newApp(t)
	bad := strings.Replace(goodDoc, "- api.example.com:443 GET", "- api.example.com GET", 1)
	form := url.Values{"base": {"0"}, "text": {bad}}
	r := a.do(hx(req{method: http.MethodPost, path: "/api/settings/check", person: "Mia", form: form}))
	r.has(t, http.StatusOK, `id="errs"`, `data-lines="5"`, "<b>Line 5</b>")
	ok := a.do(hx(req{method: http.MethodPost, path: "/api/settings/check", person: "Mia", form: url.Values{"base": {"0"}, "text": {goodDoc}}}))
	ok.has(t, http.StatusOK, `id="errs"`)
	ok.lacks(t, "<b>Line")
	if got := a.current(); got.Version != 0 {
		t.Errorf("a check saved v%d", got.Version)
	}
}

func TestATypedValueIsSealedApartAndNeverComesBack(t *testing.T) {
	a := newApp(t)
	typed := strings.Replace(goodDoc, "value: not set", "value: "+typedSecret, 1)
	r := a.do(put("Mia", 0, typed))
	r.has(t, http.StatusOK, `"replaced":["shop staging API"]`)
	r.lacks(t, typedSecret)
	for _, path := range []string{"/api/settings", "/api/settings/versions", "/settings"} {
		a.do(get("Mia", path)).lacks(t, typedSecret)
	}
	got := a.current()
	if !strings.Contains(got.Text, "value: set") {
		t.Errorf("the stored document does not show the value as set: %s", got.Text)
	}
	for _, path := range []string{"/api/settings/versions", "/api/settings/versions/1"} {
		hx(get("Mia", path))
		a.do(hx(get("Mia", path))).lacks(t, typedSecret)
	}
	a.do(get("Mia", "/api/settings/versions")).has(t, http.StatusOK, "replaced the value of shop staging API")
	a.requireSealed("shop staging API", typedSecret)
}

func (a *app) requireSealed(connection, want string) {
	a.t.Helper()
	ctx := context.Background()
	db, err := store.Open(ctx, a.dsn)
	if err != nil {
		a.t.Fatal(err)
	}
	defer db.Close()
	sec, err := db.Secret(ctx, connection)
	if err != nil {
		a.t.Fatalf("secret of %q: %v", connection, err)
	}
	if strings.Contains(string(sec.Ciphertext), want) {
		a.t.Error("the stored value is not encrypted")
	}
	box, err := secretbox.New(a.key)
	if err != nil {
		a.t.Fatal(err)
	}
	plain, err := box.Open(sec.Ciphertext, sec.KeyID)
	if err != nil || string(plain) != want {
		a.t.Errorf("the stored value opens to %q, %v, want %q", plain, err, want)
	}
}

func TestARefusedSaveDoesNotEchoATypedValueAndKeepsTheOldOne(t *testing.T) {
	a := newApp(t)
	a.do(put("Mia", 0, strings.Replace(goodDoc, "value: not set", "value: "+typedSecret, 1))).has(t, http.StatusOK)
	other := "second-value-77"
	bad := strings.Replace(goodDoc, "value: not set", "value: "+other, 1) + "extra: key\n"
	for _, r := range []req{put("Mia", 1, bad), putForm("Mia", "1", bad)} {
		a.do(r).has(t, http.StatusUnprocessableEntity)
		a.do(r).lacks(t, other)
	}
	a.requireSealed("shop staging API", typedSecret)
}

func TestASetValueKeepsItsSecretAndADeletedConnectionDropsIt(t *testing.T) {
	a := newApp(t)
	a.do(put("Mia", 0, strings.Replace(goodDoc, "value: not set", "value: "+typedSecret, 1))).has(t, http.StatusOK)
	set := a.current().Text
	a.do(put("Mia", 1, strings.Replace(set, "- api.example.com:443 GET", "- api.example.com:443 GET POST", 1))).has(t, http.StatusOK, `"version":2`)
	a.requireSealed("shop staging API", typedSecret)
	a.do(put("Mia", 2, "repositories: []\n")).has(t, http.StatusOK, `"version":3`)
	db, err := store.Open(context.Background(), a.dsn)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err := db.Secret(context.Background(), "shop staging API"); err == nil || !strings.Contains(err.Error(), domain.ErrNotFound.Error()) {
		t.Errorf("a deleted connection kept its secret: %v", err)
	}
}

func TestTheSettingsPageAndItsFragments(t *testing.T) {
	a := newApp(t)
	page := a.do(get("Mia", "/settings"))
	page.has(t, http.StatusOK, "One document for example-org. Everyone can read and edit it.", "# Settings. Everyone", `name="base" value="0"`, `id="historybtn"`, `class="cm-host"`)
	saved := a.do(putForm("Mia", "0", typedDocForPage()))
	saved.has(t, http.StatusOK, "Saved v1. The new value of shop staging API is stored; it shows as set. Running tasks get it at their next request.", `id="editorbox"`, `name="base" value="1"`)
	saved.lacks(t, typedSecret)
	if saved.header.Get("HX-Trigger") != "settings-saved" {
		t.Errorf("a save must tell an open History to refresh, HX-Trigger = %q", saved.header.Get("HX-Trigger"))
	}
	list := a.do(hx(get("Mia", "/api/settings/versions")))
	list.has(t, http.StatusOK, `id="history"`, `hx-get="/api/settings/versions/1"`, "replaced the value of shop staging API", "Mia")
	diff := a.do(hx(get("Mia", "/api/settings/versions/1")))
	diff.has(t, http.StatusOK, "saved by Mia, ", ", replaced the value of shop staging API</span>", "Back to editing", `<div class="add">+ repositories:</div>`, `aria-pressed="true"`)
	diff.lacks(t, typedSecret)
	a.do(get("Mia", "/api/settings/versions/9")).has(t, http.StatusNotFound)
	a.do(get("Mia", "/api/settings/versions/x")).has(t, http.StatusNotFound)
}

func typedDocForPage() string {
	return strings.Replace(goodDoc, "value: not set", "value: "+typedSecret, 1)
}

func TestBadBodiesAreRefusedPlainly(t *testing.T) {
	a := newApp(t)
	a.do(req{method: http.MethodPut, path: "/api/settings", person: "Mia", json: true, body: "{"}).has(t, http.StatusBadRequest)
	a.do(putForm("Mia", "x", goodDoc)).has(t, http.StatusBadRequest)
	a.do(putForm("Mia", "-1", goodDoc)).has(t, http.StatusBadRequest)
}

func TestACrossSitePostIsRefusedAndASameSiteOneIsNot(t *testing.T) {
	a := newApp(t)
	for name, headers := range map[string]map[string]string{
		"fetch metadata": {"Sec-Fetch-Site": "cross-site"},
		"foreign origin": {"Origin": "https://evil.example"},
	} {
		r := put("Mia", 0, goodDoc)
		r.headers = headers
		if got := a.do(r); got.code != http.StatusForbidden {
			t.Errorf("%s: status %d, want 403", name, got.code)
		}
		c := req{method: http.MethodPost, path: "/api/settings/check", person: "Mia", json: true, body: "{}", headers: headers}
		if got := a.do(c); got.code != http.StatusForbidden {
			t.Errorf("%s check: status %d, want 403", name, got.code)
		}
	}
	if got := a.current(); got.Version != 0 {
		t.Fatalf("a cross-site save made v%d", got.Version)
	}
	same := put("Mia", 0, goodDoc)
	same.headers = map[string]string{"Sec-Fetch-Site": "same-origin", "Origin": "http://example.com"}
	a.do(same).has(t, http.StatusOK)
}
