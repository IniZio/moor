package yamlfile

import (
	"fmt"

	"github.com/robfig/cron/v3"
)

// checkCron accepts what the cron library's standard parser accepts: five fields, or one of its descriptors.
func checkCron(expr string) error {
	if _, err := cron.ParseStandard(expr); err != nil {
		return fmt.Errorf("cron %q: %w", expr, err)
	}
	return nil
}
