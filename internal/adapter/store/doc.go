// Package store keeps tasks, their events and their result versions in Postgres.
//
// Open applies the embedded migrations, so a replica that starts first prepares the schema for the
// rest. Save is the only write. It runs in one transaction under the task row's lock.
package store
