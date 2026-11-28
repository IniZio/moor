package yamlfile

import (
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/robfig/cron/v3"

	"github.com/IniZio/moor/internal/domain"
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

// checkZone accepts an IANA time zone name. UTC is one, and Local, which names the machine's own zone, is not.
func checkZone(name string) error {
	if name == "" || name == "Local" {
		return fmt.Errorf("timezone %q is not an IANA name such as Europe/London", name)
	}
	if _, err := time.LoadLocation(name); err != nil {
		return fmt.Errorf("timezone %q is not an IANA name such as Europe/London", name)
	}
	return nil
}

// Crons reads the schedule entries of loaded specs for the clock.
type Crons struct{}

// Fires says whether the entry fires in the minute that starts at minute, and gives that minute in the entry's
// zone. A local minute that a clock change skips never comes, so it does not fire. A local minute that a clock
// change repeats comes twice, and the caller keys its firing on the local time to fire it once.
func (Crons) Fires(s domain.ScheduleOn, minute time.Time) (time.Time, bool) {
	loc := time.UTC
	if s.Timezone != "" {
		l, err := time.LoadLocation(s.Timezone)
		if err != nil {
			return time.Time{}, false
		}
		loc = l
	}
	sched, err := cron.ParseStandard(s.Cron)
	if err != nil {
		return time.Time{}, false
	}
	local := minute.Truncate(time.Minute).In(loc)
	return local, sched.Next(local.Add(-time.Second)).Equal(local)
}
