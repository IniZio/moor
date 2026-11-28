// Package light switches over an enum and misses a case.
package light

type Light uint8

const (
	Red Light = iota + 1
	Green
)

func Go(l Light) bool {
	switch l {
	case Green:
		return true
	}
	return false
}
