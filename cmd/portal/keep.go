package main

import "github.com/IniZio/moor/internal/task"

// withOperations gives the task service the keep window and the time limit the operator set.
func withOperations(c task.Config, cfg config) task.Config {
	c.KeepWindow, c.TimeLimit = cfg.keepWindow, cfg.timeLimit
	return c
}
