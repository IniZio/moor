package link

import "github.com/IniZio/moor/internal/wire"

// Verdict is what a slot does with a launch aimed at its task.
type Verdict uint8

const (
	Start Verdict = iota + 1
	Join
	Refuse
)

// Newer reports whether a aims at a later box generation than b, or at a later run of the same one.
func Newer(a, b wire.Fence) bool {
	return a.Gen > b.Gen || (a.Gen == b.Gen && a.Run > b.Run)
}

// Admit decides a launch k against the fence of the job running in the task's slot, nil when none
// runs. An equal fence joins the running job, an older one is refused, a newer one replaces it. A
// fence of another task is refused: a slot holds one task.
func Admit(running *wire.Fence, k wire.Fence) Verdict {
	switch {
	case running == nil:
		return Start
	case running.Task != k.Task:
		return Refuse
	case *running == k:
		return Join
	case Newer(*running, k):
		return Refuse
	}
	return Start
}
