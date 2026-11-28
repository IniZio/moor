package domain_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain"
)

func TestCheckCron(t *testing.T) {
	for _, ok := range []string{
		"0 6 * * 1", "*/15 * * * *", "0 0 1 1 *", "0 9-17 * * mon-fri", "5,10,15 * * * *", "0 0 * JAN,feb sun",
		"0-30/10 * * * *", "0 0 * * 7", "  0   6 * * 1 ", "1-5 */2 * * *",
	} {
		if err := domain.CheckCron(ok); err != nil {
			t.Errorf("%q: %v", ok, err)
		}
	}
	for _, bad := range []string{
		"", "* * * *", "* * * * * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "* * 32 * *", "* * * 0 *", "* * * 13 *",
		"* * * * 8", "5-1 * * * *", "*/0 * * * *", "*/x * * * *", "a * * * *", "-1 * * * *", "+1 * * * *", "1-2-3 * * * *",
		", * * * *", "1, * * * *", "* * * foo *", "* * * * mon-xyz", "@daily",
	} {
		if err := domain.CheckCron(bad); err == nil {
			t.Errorf("%q: accepted", bad)
		}
	}
}
