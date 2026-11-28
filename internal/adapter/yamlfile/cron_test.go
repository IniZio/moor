package yamlfile_test

import (
	"fmt"
	"strconv"
	"testing"

	"github.com/IniZio/moor/internal/adapter/yamlfile"
)

// parseCron reads a spec whose only schedule is expr, so a cron is judged the way a person's file is.
func parseCron(expr string) error {
	_, err := yamlfile.ParseTaskSpec([]byte("name: a\non:\n  schedule:\n    - cron: " + strconv.Quote(expr) + "\nprompt: p\n"))
	if err != nil {
		return fmt.Errorf("spec with cron %q: %w", expr, err)
	}
	return nil
}

// The library's behaviour is the rule for what a cron may say.
func TestCronIsReadByTheCronLibrary(t *testing.T) {
	for _, ok := range []string{
		"0 6 * * 1", "*/15 * * * *", "0 0 1 1 *", "0 9-17 * * mon-fri", "5,10,15 * * * *", "0 0 * JAN,feb sun",
		"0-30/10 * * * *", "  0   6 * * 1 ", "1-5 */2 * * *",
		// accepted by the library, refused by the hand-written check this replaced
		"+1 * * * *", ", * * * *", "1, * * * *", "@daily",
	} {
		if err := parseCron(ok); err != nil {
			t.Errorf("%q: %v", ok, err)
		}
	}
	for _, bad := range []string{
		"", "* * * *", "* * * * * *", "0 0 9 * * 1", "60 * * * *", "* 24 * * *", "* * 0 * *", "* * 32 * *", "* * * 0 *", "* * * 13 *",
		"* * * * 8", "5-1 * * * *", "*/0 * * * *", "*/x * * * *", "a * * * *", "-1 * * * *", "1-2-3 * * * *",
		"* * * foo *", "* * * * mon-xyz",
		// refused by the library, accepted by the hand-written check this replaced
		"0 0 * * 7",
	} {
		if err := parseCron(bad); err == nil {
			t.Errorf("%q: accepted", bad)
		}
	}
}
