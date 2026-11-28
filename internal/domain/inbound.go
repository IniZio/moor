package domain

import (
	"errors"
	"fmt"
	"time"
)

// What Run can refuse.
var (
	// ErrBadLink is a Run whose text is not a link the spec's event source reads.
	ErrBadLink = errors.New("not a link the event source reads")
	// ErrNoSpec is a Run of a spec the team's home does not have, or whose file is broken.
	ErrNoSpec = errors.New("no such task spec")
	// ErrNotRunnable is a Run of a spec that lists no workflow_dispatch.
	ErrNotRunnable = errors.New("the spec has no workflow_dispatch")
	// ErrRunUnsupported is a Run the product does not start yet.
	ErrRunUnsupported = errors.New("this kind of Run is not built yet")
)

// NotAllowedError is an event or a Run on a repository that Settings does not list, or that the spec leaves out.
type NotAllowedError struct{ Repo Repo }

func (e *NotAllowedError) Error() string {
	return fmt.Sprintf("repository %s is not allowed for this spec", e.Repo)
}

// InboundEvent is one delivery of a source's webhook as it is stored. The source and the delivery id are its
// key, so a redelivery finds it and starts nothing. Outcome says what the portal did with it.
type InboundEvent struct {
	Source     EventSource
	DeliveryID string
	// Event is the source's own name for what happened, such as pull_request, and Action its kind.
	Event      string
	Action     string
	ReceivedAt time.Time
	Outcome    string
}

// Occurrence is an event a source reported, read by that source's adapter into what the product needs.
// Event.Context and Incoming.Payload hold the same JSON, the fields the prompt templates and the screens read.
// Actor is the person behind it, such as the pull request's sender.
type Occurrence struct {
	Incoming Incoming
	Event    Event
	Actor    string
	// Ref and Commit are what the task checks out. Ref is fetched first, so a commit no branch holds, such as a
	// fork's, resolves. Both are empty for an event that names no commit: the task checks out the default branch.
	Ref    string
	Commit Commit
}

// Inbound is one webhook delivery after its adapter has checked and read it. Occurrence is nil when the
// adapter set the delivery aside, and Ignored then says why.
type Inbound struct {
	Source     EventSource
	DeliveryID string
	Event      string
	Action     string
	Occurrence *Occurrence
	Ignored    string
}

// InboundResult is what the portal did with a delivery. Duplicate is true when the delivery id was seen
// before. Tasks are the tasks the delivery started, or the ones the first delivery of it had started.
type InboundResult struct {
	Duplicate bool
	Tasks     []TaskID
	Outcome   string
}

// EventText is what a source's adapter reads of an event for the screens. Verb is "opened" or "updated" for an
// event that came in as a webhook, and empty for one a person made with Run.
type EventText struct {
	Title   string
	Subject string
	Verb    string
	Actor   string
	URL     string
}

// EventReader reads the events of one source for the screens.
type EventReader interface {
	ReadEvent(Event) (EventText, bool)
}

// EventReaders picks the reader of an event's source.
type EventReaders map[EventSource]EventReader

// Read reads an event with its source's reader, and gives false when the source has none or cannot read it.
func (r EventReaders) Read(ev Event) (EventText, bool) {
	reader, ok := r[ev.Source]
	if !ok {
		return EventText{}, false
	}
	return reader.ReadEvent(ev)
}

// DeliveryTarget is a place an answer goes. Label is how a person names it, such as "pull request #90".
type DeliveryTarget struct {
	Kind  string
	Ref   string
	Label string
}

// Section is what one task spec shows in a delivery: its newest task, that task's revision and the version
// shown, with the line and the page the version has.
type Section struct {
	Task    TaskID
	Rev     Revision
	Version int
	Line    string
	URL     string
}

// Delivery is one place an answer goes, shared by the tasks of every spec that answers it. ExternalID names
// what the source made, such as a comment, and is empty until it exists.
type Delivery struct {
	Kind       string
	Ref        string
	ExternalID string
	Sections   map[SpecName]Section
	UpdatedAt  time.Time
}
