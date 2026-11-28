package domain_test

import (
	"errors"
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestParseRepo(t *testing.T) {
	ok := map[string]domain.Repo{
		"example-org/shop-web":            {Host: "github.com", Path: "example-org/shop-web"},
		"Example-Org/Shop-Web":            {Host: "github.com", Path: "example-org/shop-web"},
		"my.org/repo":                     {Host: "github.com", Path: "my.org/repo"},
		"forge.example/example-org/shop":  {Host: "forge.example", Path: "example-org/shop"},
		"Forge.Example/group/sub/project": {Host: "forge.example", Path: "group/sub/project"},
		"localhost/example-org/shop":      {Host: "localhost", Path: "example-org/shop"},
		"forge.example/a":                 {Host: "github.com", Path: "forge.example/a"},
	}
	for in, want := range ok {
		got, err := domain.ParseRepo(in)
		if err != nil || got != want {
			t.Errorf("%q: got %v, %v, want %v", in, got, err, want)
		}
	}
	for _, in := range []string{
		"", "shop", "/shop", "a/", "a//b", "a/b/", "a b/c", " a/b", "a/b ", "a/..", "./b",
		"a/b/c", "github/a/b", "forge.example/", "a/b?x", "a/b\n",
	} {
		if _, err := domain.ParseRepo(in); !errors.Is(err, domain.ErrBadRepo) {
			t.Errorf("%q: err = %v", in, err)
		}
	}
}
