package domain

import "time"

// ResultVersion is one published result page of a task. Digest is unique within a task.
type ResultVersion struct {
	Task        TaskID
	N           int
	EntryURL    string
	Line        string
	Digest      string
	PublishedAt time.Time
}
