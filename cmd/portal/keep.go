package main

import "github.com/IniZio/moor/internal/task"

// withKeep gives the task service the keep window the operator set.
func withKeep(c task.Config, cfg config) task.Config {
	c.KeepWindow = cfg.keepWindow
	return c
}
