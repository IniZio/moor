// Package task starts, supersedes, ends and publishes a task. It is pure logic over a Store and a
// Clock, so a caller runs each operation again after any failure and gets the same end state.
package task
