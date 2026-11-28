package domain

import (
	"encoding/json"
	"time"
)

// EventKind is the kind column of a task_events row, which says how to read its data.
type EventKind string

const (
	KindState     EventKind = "state"
	KindNote      EventKind = "note"
	KindBox       EventKind = "box"
	KindHome      EventKind = "home"
	KindPublished EventKind = "published"
	KindDelivered EventKind = "delivered"
	KindPolicy    EventKind = "policy"
	KindError     EventKind = "error"
)

// EventRow is one row of a task's history. Seq is strictly increasing within the task.
type EventRow struct {
	Task TaskID
	Seq  int64
	At   time.Time
	Kind EventKind
	Data json.RawMessage
}

type StateData struct {
	From   string `json:"from"`
	To     string `json:"to"`
	Reason string `json:"reason"`
}

type NoteData struct {
	Text string `json:"text"`
}

type BoxData struct {
	Gen        int64  `json:"gen"`
	HomeCommit string `json:"home_commit"`
}

type PublishedData struct {
	Version  int    `json:"version"`
	Line     string `json:"line"`
	EntryURL string `json:"entry_url"`
}

type DeliveredData struct {
	Target     string `json:"target"`
	ExternalID string `json:"external_id"`
	Version    int    `json:"version"`
}

type ErrorData struct {
	Text string `json:"text"`
}
