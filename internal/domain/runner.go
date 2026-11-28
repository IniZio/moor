package domain

import "errors"

// ErrUnknownRunner is what a runner key lookup returns for an id with no record, or a revoked one.
var ErrUnknownRunner = errors.New("unknown runner")
