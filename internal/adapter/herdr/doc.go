// Package herdr is a client for herdr-core (the terminal host, command hc): JSON-RPC 2.0, protocol
// generation 1, one message per line, over any byte stream such as the stdio of `hc connect`.
// It knows no box and no task. The transcripts in testdata come from hc 0.1.16 and must be
// re-recorded against the pinned herdr-core version (make herdr-golden).
package herdr
