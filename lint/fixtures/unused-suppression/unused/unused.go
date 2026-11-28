// Package unused suppresses a linter that reports nothing there.
package unused

func Sum(a, b int) int {
	return a + b //nolint:errcheck // nothing here returns an error
}
