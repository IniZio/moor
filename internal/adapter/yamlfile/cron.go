package yamlfile

import (
	"errors"
	"fmt"
	"strings"

	"github.com/robfig/cron/v3"
)

var errCronVocabulary = errors.New("a schedule is five fields, such as 0 6 * * 1, with no @descriptor and no time zone prefix")

// checkCron accepts what the cron library's standard parser accepts, less what GitHub Actions does not say: a
// descriptor such as @daily or @every 5m, and a TZ= or CRON_TZ= prefix.
func checkCron(expr string) error {
	if usesLibraryOnlySyntax(expr) {
		return fmt.Errorf("cron %q: %w", expr, errCronVocabulary)
	}
	if _, err := cron.ParseStandard(expr); err != nil {
		return fmt.Errorf("cron %q: %w", expr, err)
	}
	return nil
}

func usesLibraryOnlySyntax(expr string) bool {
	text := strings.ToUpper(strings.TrimSpace(expr))
	return strings.HasPrefix(text, "@") || strings.HasPrefix(text, "TZ=") || strings.HasPrefix(text, "CRON_TZ=")
}
