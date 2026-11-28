// Package inbound turns what an event source reports into tasks. A webhook delivery is stored once by its id and
// starts one task for each task spec that hears it, all in one transaction, and a Run by hand starts the task
// that the same event would. It reads the specs and the settings through ports, and knows no source.
package inbound
