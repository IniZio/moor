// Package answer posts a task's result back to the event that started it. When a publish leaves the task owing a
// delivery, it writes the one comment of that place, creating it the first time and editing it after, with one
// section for each task spec. The product posts, never the agent. A failed post is recorded on the task with its
// reason and is not tried again by itself: the publish, made again with the same digest, owes the delivery again.
package answer
