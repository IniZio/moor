// Package runner runs tasks on the host. It dials out to the portal, serves the ops the portal
// opens on the link, and keeps no state of its own: what it holds per task is what the host's
// container labels show, so a restarted runner re-adopts its tasks by listing them.
package runner
