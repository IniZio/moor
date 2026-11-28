// Package tasktest holds the in-memory Store and the fixed Clock that tests of task logic, and of
// the code on top of it, share.
package tasktest

import "time"

// Clock is a Clock that moves only when told.
type Clock struct{ now time.Time }

func NewClock(start time.Time) *Clock { return &Clock{now: start} }

func (c *Clock) Now() time.Time { return c.now }

func (c *Clock) Advance(d time.Duration) { c.now = c.now.Add(d) }
