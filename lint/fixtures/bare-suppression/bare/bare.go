// Package bare suppresses without naming a linter or a reason.
package bare

import "os"

func Remove() {
	os.Remove("x") //nolint
}
