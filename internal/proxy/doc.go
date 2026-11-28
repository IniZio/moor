// Package proxy is a task's proxy: the Box egress proxy or the desk proxy. It reads the task's policy from
// the portal, refuses what the policy does not allow with a 403 that says why, reports each refusal, and adds
// connection values on the wire. elazarl/goproxy does the proxying and the TLS interception.
package proxy
