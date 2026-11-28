package main

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"regexp"
	"testing"
)

type machineRow struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	State      string `json:"state"`
	Running    int    `json:"running"`
	EnrolledBy string `json:"enrolled_by"`
	Controls   bool   `json:"controls"`
}

var joinCommand = regexp.MustCompile(`^runner join -portal (\S+) -id (\S+) -token (\S+) -key runner\.key$`)

// addMachine enrols a machine as person and returns its id and one-time token, read from the join command.
func (a *app) addMachine(person string) (id, token string) {
	a.t.Helper()
	r := a.do(req{method: http.MethodPost, path: "/api/machines", person: person})
	r.has(a.t, http.StatusCreated)
	var out struct {
		Machine     machineRow `json:"machine"`
		JoinCommand string     `json:"join_command"`
	}
	if err := json.Unmarshal([]byte(r.body), &out); err != nil {
		a.t.Fatal(err)
	}
	m := joinCommand.FindStringSubmatch(out.JoinCommand)
	if m == nil || m[2] != out.Machine.ID || out.Machine.State != "joining" || out.Machine.EnrolledBy != person {
		a.t.Fatalf("add machine gave %+v", out)
	}
	a.joinedPortal = m[1]
	return m[2], m[3]
}

func joinBody(t *testing.T, id, token, name string) (string, ed25519.PrivateKey) {
	t.Helper()
	pub, key, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(map[string]string{"id": id, "token": token, "name": name, "public_key": base64.StdEncoding.EncodeToString(pub)})
	return string(b), key
}

func joinReq(body string) req {
	return req{method: http.MethodPost, path: "/runner/join", json: true, body: body}
}

func (a *app) machines(person string) map[string]machineRow {
	a.t.Helper()
	r := a.do(get(person, "/api/machines"))
	r.has(a.t, http.StatusOK)
	var out struct {
		Machines []machineRow `json:"machines"`
	}
	if err := json.Unmarshal([]byte(r.body), &out); err != nil {
		a.t.Fatal(err)
	}
	rows := map[string]machineRow{}
	for _, m := range out.Machines {
		rows[m.ID] = m
	}
	return rows
}

func withPublicURL(c *config) { c.publicURL = "https://portal.example.com" }

func TestAMemberAddsAMachineAndTheRunnerJoinsOnceWithTheToken(t *testing.T) {
	a := newApp(t, withPublicURL)
	id, token := a.addMachine("Mia")
	if a.joinedPortal != "https://portal.example.com" {
		t.Errorf("the join command dials %q", a.joinedPortal)
	}
	if got := a.machines("Ken")[id]; got.State != "joining" || got.EnrolledBy != "Mia" || got.Controls {
		t.Errorf("another member sees %+v", got)
	}
	body, _ := joinBody(t, id, "wrong-"+token, "host-a")
	a.do(joinReq(body)).has(t, http.StatusForbidden)
	body, _ = joinBody(t, id, token, "host-a")
	a.do(joinReq(body)).has(t, http.StatusOK, `"id":"`+id+`"`)
	a.do(joinReq(body)).has(t, http.StatusForbidden)
	if got := a.machines("Mia")[id]; got.Name != "host-a" || got.State != "offline" || !got.Controls {
		t.Errorf("after the join the enroller sees %+v", got)
	}
	other, otherToken := a.addMachine("Ken")
	body, _ = joinBody(t, other, otherToken, "host-a")
	a.do(joinReq(body)).has(t, http.StatusConflict, "host-a")
	for _, bad := range []string{"{", `{"id":"x"}`, `{"id":"x","token":"t","name":"n","public_key":"AAAA"}`, `{"id":"x","token":"t","name":"n","public_key":"","extra":1}`} {
		a.do(joinReq(bad)).has(t, http.StatusBadRequest)
	}
}

func TestOnlyTheEnrollerActsOnAMachineAndRemoveNeedsARevoke(t *testing.T) {
	a := newApp(t, withPublicURL)
	id, token := a.addMachine("Mia")
	body, _ := joinBody(t, id, token, "host-a")
	a.do(joinReq(body)).has(t, http.StatusOK)
	act := func(person, action string) reply {
		if action == "remove" {
			return a.do(req{method: http.MethodDelete, path: "/api/machines/" + id, person: person})
		}
		return a.do(req{method: http.MethodPost, path: "/api/machines/" + id + "/" + action, person: person})
	}
	for _, action := range []string{"drain", "disconnect", "revoke", "remove"} {
		act("Ken", action).has(t, http.StatusForbidden)
	}
	act("Mia", "remove").has(t, http.StatusConflict)
	act("Mia", "drain").has(t, http.StatusNoContent)
	if got := a.machines("Mia")[id]; got.State != "draining" {
		t.Errorf("after drain: %+v", got)
	}
	act("Mia", "disconnect").has(t, http.StatusNoContent)
	act("Mia", "revoke").has(t, http.StatusNoContent)
	if got := a.machines("Mia")[id]; got.State != "revoked" {
		t.Errorf("after revoke: %+v", got)
	}
	act("Mia", "remove").has(t, http.StatusNoContent)
	if _, ok := a.machines("Mia")[id]; ok {
		t.Error("a removed machine is still listed")
	}
	act("Mia", "drain").has(t, http.StatusNotFound)
	a.do(req{method: http.MethodPost, path: "/api/machines/" + id + "/reboot", person: "Mia"}).has(t, http.StatusNotFound)
}

func TestTheMachineRoutesNeedAPerson(t *testing.T) {
	a := newApp(t, withPublicURL)
	for _, r := range []req{
		get("", "/api/machines"),
		{method: http.MethodPost, path: "/api/machines"},
		{method: http.MethodPost, path: "/api/machines/m1/drain"},
		{method: http.MethodDelete, path: "/api/machines/m1"},
	} {
		a.do(r).has(t, http.StatusUnauthorized)
	}
}
