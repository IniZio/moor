package domain

import (
	"errors"
	"time"
)

// A machine is a Docker host that runs tasks. Its RunnerID is the machine's id.

var (
	// ErrNotEnroller is a row action by a member other than the one who enrolled the machine.
	ErrNotEnroller = errors.New("only the member who enrolled the machine controls it")
	// ErrJoinRefused is a join whose token is unknown, used, expired or for another machine.
	ErrJoinRefused = errors.New("the join token is not valid")
	// ErrNotDraining is a resume of a machine that is not draining.
	ErrNotDraining = errors.New("only a draining machine can be resumed")
	// ErrNotRevoked is a remove of a machine that is not revoked.
	ErrNotRevoked = errors.New("only a revoked machine can be removed")
)

// MachineState is what a machine row shows. It is derived from the row and the clock, never stored.
type MachineState string

const (
	// MachineJoining is a machine whose join command has not been run yet.
	MachineJoining  MachineState = "joining"
	MachineLinked   MachineState = "linked"
	MachineDraining MachineState = "draining"
	MachineOffline  MachineState = "offline"
	MachineRevoked  MachineState = "revoked"
)

// LinkFresh is how long after the holder last heard the runner the machine still counts as linked.
const LinkFresh = 15 * time.Second

// Machine is one row of the machines table, plus the count of tasks it runs.
type Machine struct {
	ID         RunnerID
	Name       string
	EnrolledBy Person
	PubKey     []byte
	Build      string
	// LinkGen orders links: each new link bumps it, and so do disconnect and revoke, which ends
	// the link of any older generation. Holder names the portal replica holding the newest link,
	// empty when no replica holds one.
	LinkGen       int64
	Holder        string
	HeardAt       time.Time
	DrainingSince time.Time
	RevokedAt     time.Time
	CreatedAt     time.Time
	// Running counts the tasks on this machine that are starting, running or delivering.
	Running int
}

// State reads the row at now. A revoke outranks a drain, and a drain outranks the link.
func (m Machine) State(now time.Time) MachineState {
	switch {
	case !m.RevokedAt.IsZero():
		return MachineRevoked
	case len(m.PubKey) == 0:
		return MachineJoining
	case !m.DrainingSince.IsZero():
		return MachineDraining
	case m.Holder != "" && now.Sub(m.HeardAt) <= LinkFresh:
		return MachineLinked
	}
	return MachineOffline
}

// MachineAdd is a new machine row with its one-time join token, stored hashed.
type MachineAdd struct {
	ID            RunnerID
	EnrolledBy    Person
	JoinHash      []byte
	JoinExpiresAt time.Time
	CreatedAt     time.Time
}

// MachineJoin burns the join token whose hash is JoinHash and stores the runner's key and name.
type MachineJoin struct {
	ID       RunnerID
	JoinHash []byte
	Name     string
	PubKey   []byte
	At       time.Time
}

// MachineAction is a row action, allowed only to the member who enrolled the machine.
type MachineAction string

const (
	Drain      MachineAction = "drain"
	Resume     MachineAction = "resume"
	Disconnect MachineAction = "disconnect"
	Revoke     MachineAction = "revoke"
	Remove     MachineAction = "remove"
)

// Enrolled is a new machine and the command that joins it, which is shown once, until ExpiresAt.
type Enrolled struct {
	Machine     Machine
	JoinCommand string
	ExpiresAt   time.Time
}

// MachineActed is what a row action did: the machine as it was before, and how many tasks a revoke failed.
type MachineActed struct {
	Machine Machine
	Failed  int
}
