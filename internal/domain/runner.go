package domain

import "errors"

var (
	// ErrUnknownRunner is what a runner key lookup returns for an id with no record.
	ErrUnknownRunner = errors.New("unknown runner")
	// ErrRevokedRunner is what a runner key lookup returns, with the key, for a revoked runner.
	ErrRevokedRunner = errors.New("revoked runner")
)
