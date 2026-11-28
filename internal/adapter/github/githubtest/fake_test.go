package githubtest

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"
)

func TestTheFakeListsItsCallsForAProcessThatCannotAskTheGoValue(t *testing.T) {
	f := NewForTest(t)
	post := func(path string) {
		resp, err := http.Post(f.URL()+path, "application/json", strings.NewReader(`{"body":"hello"}`))
		if err != nil {
			t.Fatal(err)
		}
		_ = resp.Body.Close()
	}
	post("/repos/example-org/shop-web/issues/90/comments")
	post("/repos/example-org/shop-web/issues/90/comments")
	resp, err := http.Get(f.URL() + "/_fake/calls")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	var calls []Call
	if err := json.NewDecoder(resp.Body).Decode(&calls); err != nil {
		t.Fatal(err)
	}
	posts := 0
	for _, c := range calls {
		if c.Method == http.MethodPost && strings.HasSuffix(c.Path, "/comments") {
			posts++
		}
	}
	if posts != 2 {
		t.Fatalf("calls = %+v", calls)
	}
}
