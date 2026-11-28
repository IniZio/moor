package domain_test

import (
	"errors"
	"reflect"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestParseHostPort(t *testing.T) {
	ok := map[string]domain.HostPort{
		"registry.npmjs.org":          {Host: "registry.npmjs.org", Port: 443},
		"*.Staging.shop.example:5432": {Host: "*.staging.shop.example", Port: 5432},
		"localhost:1":                 {Host: "localhost", Port: 1},
		"10.0.0.1:65535":              {Host: "10.0.0.1", Port: 65535},
	}
	for in, want := range ok {
		got, err := domain.ParseHostPort(in)
		if err != nil || got != want {
			t.Errorf("%q: got %+v, %v, want %+v", in, got, err, want)
		}
	}
	for _, in := range []string{"", ":443", "example.com:", "example.com:0", "example.com:65536", "example.com:http", "*:443", "*.", "a*.example.com", "https://example.com", "example.com/path"} {
		if _, err := domain.ParseHostPort(in); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("%q: err = %v", in, err)
		}
	}
	if s := (domain.HostPort{Host: "a.example", Port: 443}).String(); s != "a.example" {
		t.Errorf("the default port prints as %q", s)
	}
}

func TestParseEntry(t *testing.T) {
	ok := map[string]domain.Entry{
		"GET /**":                                {Methods: []domain.Method{"GET"}, Path: "/**"},
		"GET|POST /repos/{repo}/pulls":           {Methods: []domain.Method{"GET", "POST"}, Path: "/repos/{repo}/pulls"},
		"DELETE /repos/{repo}/issues/*/labels/*": {Methods: []domain.Method{"DELETE"}, Path: "/repos/{repo}/issues/*/labels/*"},
		"GET /{repo}.git/info/refs":              {Methods: []domain.Method{"GET"}, Path: "/{repo}.git/info/refs"},
	}
	for in, want := range ok {
		got, err := domain.ParseEntry(in)
		if err != nil || !reflect.DeepEqual(got, want) || got.String() != in {
			t.Errorf("%q: got %+v, %v, want %+v", in, got, err, want)
		}
	}
	for _, in := range []string{
		"", "GET", "/x", "get /x", "GET|GET /x", "FETCH /x", "GET x", "GET /x?y=1", "GET /a b", "GET /a**", "GET /{owner}/x",
		"GET /[ab]", "GET /../x", "GET /a/./b", "GET| /x", "* /x",
	} {
		if _, err := domain.ParseEntry(in); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("%q: err = %v", in, err)
		}
	}
}

func TestValidateRefusesWhatTheParserWouldNot(t *testing.T) {
	app := domain.Credential{Name: "github", App: &domain.GitHubApp{AppID: 1, PrivateKey: domain.SecretSet, WebhookSecret: domain.SecretNotSet}}
	good := domain.Rule{
		Name: "gh", Hosts: []domain.HostPort{{Host: "api.github.com", Port: 443}}, Credential: "github",
		Inject: domain.HeaderInject{Name: "Authorization", Form: "token {secret}"}, Allow: domain.Access{Any: true},
	}
	if err := (domain.Settings{Credentials: []domain.Credential{app}, Rules: []domain.Rule{good}}).Validate(); err != nil {
		t.Fatalf("a whole document: %v", err)
	}
	noCred, noInject, noAllow, both := good, good, good, app
	noCred.Credential = "other"
	noInject.Inject = nil
	noAllow.Allow = domain.Access{}
	both.Value = domain.SecretSet
	for name, s := range map[string]domain.Settings{
		"an unknown credential":   {Credentials: []domain.Credential{app}, Rules: []domain.Rule{noCred}},
		"a credential, no inject": {Credentials: []domain.Credential{app}, Rules: []domain.Rule{noInject}},
		"no allow":                {Credentials: []domain.Credential{app}, Rules: []domain.Rule{noAllow}},
		"value and app":           {Credentials: []domain.Credential{both}},
		"a repeated rule":         {Credentials: []domain.Credential{app}, Rules: []domain.Rule{good, good}},
	} {
		if err := s.Validate(); !errors.Is(err, domain.ErrBadSettings) {
			t.Errorf("%s: err = %v", name, err)
		}
	}
}

func TestSecretKeysAreTheSetFieldsOfEachCredential(t *testing.T) {
	s := domain.Settings{Credentials: []domain.Credential{
		{Name: "github", App: &domain.GitHubApp{AppID: 1, PrivateKey: domain.SecretSet, WebhookSecret: domain.SecretSet}},
		{Name: "anthropic", Value: domain.SecretSet},
		{Name: "later", Value: domain.SecretNotSet},
	}}
	want := []string{"github/private_key", "github/webhook_secret", "anthropic/value"}
	if got := s.SecretKeys(); !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}
