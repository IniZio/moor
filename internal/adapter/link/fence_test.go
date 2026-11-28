package link

import (
	"testing"

	"github.com/IniZio/moor/internal/wire"
)

func TestAdmitJoinsEqualRefusesOlderStartsNewer(t *testing.T) {
	cur := wire.Fence{Task: "t1", Run: 2, Gen: 3}
	cases := []struct {
		name    string
		running *wire.Fence
		k       wire.Fence
		want    Verdict
	}{
		{"nothing runs", nil, cur, Start},
		{"equal key", &cur, cur, Join},
		{"older run", &cur, wire.Fence{Task: "t1", Run: 1, Gen: 3}, Refuse},
		{"older gen with a later run", &cur, wire.Fence{Task: "t1", Run: 9, Gen: 2}, Refuse},
		{"newer run", &cur, wire.Fence{Task: "t1", Run: 3, Gen: 3}, Start},
		{"newer gen with an earlier run", &cur, wire.Fence{Task: "t1", Run: 1, Gen: 4}, Start},
		{"another task", &cur, wire.Fence{Task: "t2", Run: 2, Gen: 3}, Refuse},
	}
	for _, c := range cases {
		if got := Admit(c.running, c.k); got != c.want {
			t.Errorf("%s: got %d, want %d", c.name, got, c.want)
		}
	}
}
