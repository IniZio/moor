// Package wire holds the runner link messages shared by the portal and the runner.
package wire

import (
	"encoding/json"
	"fmt"
)

// Proto is the link protocol version this build speaks.
const Proto = 1

type Kind string

const (
	KindHello          Kind = "hello"
	KindChallenge      Kind = "challenge"
	KindProof          Kind = "proof"
	KindWelcome        Kind = "welcome"
	KindUpdateRequired Kind = "update_required"
	KindRefused        Kind = "refused"
	KindRelink         Kind = "relink"
	KindRunnerStatus   Kind = "runner.status"
	KindRunnerReport   Kind = "runner.report"
	KindTaskStart      Kind = "task.start"
	KindTaskWatch      Kind = "task.watch"
	KindBoxRecreate    Kind = "box.recreate"
	KindTaskStop       Kind = "task.stop"
	KindTaskRelease    Kind = "task.release"
	KindTaskList       Kind = "task.list"
	KindTermAttach     Kind = "term.attach"
	KindTermRead       Kind = "term.read"
	KindTermSend       Kind = "term.send"
	KindWarmBuild      Kind = "warm.build"
)

// Code classifies a failure that crossed the link. A code is never renumbered and never reused.
type Code string

const (
	CodeUnknownOp  Code = "unknown_op"
	CodeBadRequest Code = "bad_request"
	CodeForbidden  Code = "forbidden"
	CodeBusy       Code = "busy"
	CodeDraining   Code = "draining"
	CodeCancelled  Code = "cancelled"
	CodeRefused    Code = "refused"
	CodeInternal   Code = "internal"
)

// Error is a failure the peer reported. errors.Is(err, &Error{Code: c}) matches on the code alone.
type Error struct {
	Code Code   `json:"code"`
	Msg  string `json:"msg"`
}

func (e *Error) Error() string { return fmt.Sprintf("%s: %s", e.Code, e.Msg) }

func (e *Error) Is(target error) bool {
	t, ok := target.(*Error)
	return ok && t.Code == e.Code
}

// TermFrame is the first byte of each frame on a term.attach stream after its opening frame.
type TermFrame byte

const (
	TermIn TermFrame = iota + 1
	TermOut
	TermResize
	TermExit
)

// Fence names one launch of a task: a job with an older run or gen is refused.
type Fence struct {
	Task string `json:"task"`
	Run  int64  `json:"run"`
	Gen  int64  `json:"gen"`
}

type Hello struct {
	Runner string `json:"runner"`
	Proto  int    `json:"proto"`
	Build  string `json:"build"`
}

type Challenge struct {
	Nonce []byte `json:"nonce"`
}

type Proof struct {
	Sig []byte `json:"sig"`
}

type Welcome struct {
	Proto int   `json:"proto"`
	Gen   int64 `json:"gen"`
}

type UpdateRequired struct {
	ServerProto int `json:"server_proto"`
	MinProto    int `json:"min_proto"`
}

type Refused struct {
	Reason string `json:"reason"`
}

type Relink struct{}

type RunnerStatus struct{}

type RunnerStatusReply struct {
	Build string `json:"build"`
	Proto int    `json:"proto"`
	Slots int    `json:"slots"`
	Tasks int    `json:"tasks"`
}

type RunnerReport struct {
	DiskFree  uint64 `json:"disk_free"`
	MemFree   uint64 `json:"mem_free"`
	SlotsFree int    `json:"slots_free"`
}

// Repo is a repository's host and its path there.
type Repo struct {
	Host string `json:"host"`
	Path string `json:"path"`
}

// Event is the integration event that started a task. Its parts are opaque to the link.
type Event struct {
	Source  string          `json:"source"`
	Group   string          `json:"group"`
	Rev     string          `json:"rev"`
	Context json.RawMessage `json:"context,omitempty"`
}

// Spec names the task spec that started a task, and the spec's file in the team's home.
type Spec struct {
	Name string `json:"name"`
	File string `json:"file"`
}

type TaskStart struct {
	Fence
	Repo          Repo            `json:"repo"`
	Commit        string          `json:"commit"`
	Setup         string          `json:"setup,omitempty"`
	Prompt        string          `json:"prompt"`
	ReviewPrompt  string          `json:"review_prompt,omitempty"`
	Skills        []string        `json:"skills"`
	Spec          *Spec           `json:"spec,omitempty"`
	Event         *Event          `json:"event,omitempty"`
	Harness       string          `json:"harness"`
	WarmState     string          `json:"warm_state"`
	BudgetSeconds int64           `json:"budget"`
	ModelToken    string          `json:"model_token"`
	Proxies       ProxyAccess     `json:"proxies"`
	SecretRules   json.RawMessage `json:"secret_rules"`
}

type TaskWatch struct{}

// SessionPart is the part of a task.watch line about the harness session, and SessionFinished its
// state once the harness exited.
const (
	SessionPart     = "session"
	SessionFinished = "finished"
)

type TaskWatchLine struct {
	Fence
	Part  string `json:"part"`
	State string `json:"state"`
	Code  int    `json:"code"`
}

type BoxRecreate struct {
	Fence
	HomeCommit string `json:"home_commit"`
	WarmState  string `json:"warm_state"`
}

type TaskStop struct {
	Fence
}

type TaskRelease struct {
	Task string `json:"task"`
}

type TaskList struct{}

type TaskListReply struct {
	Tasks []ListedTask `json:"tasks"`
}

type ListedTask struct {
	Fence
	Containers []string `json:"containers"`
}

type TermAttach struct {
	Task    string `json:"task"`
	Mode    string `json:"mode"`
	Cols    uint16 `json:"cols"`
	Rows    uint16 `json:"rows"`
	History int    `json:"history"`
}

type WarmBuild struct {
	Repo          Repo   `json:"repo"`
	Commit        string `json:"commit"`
	Setup         string `json:"setup,omitempty"`
	InputHash     string `json:"input_hash"`
	HomeCommit    string `json:"home_commit"`
	BudgetSeconds int64  `json:"budget"`
}
