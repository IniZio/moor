// Package github is the adapter for GitHub pull requests. It checks and reads a pull request webhook into the
// domain's event, reads a pull request link the same way, and answers an event with one comment that it creates
// and then edits in place. The signature check, the payload types and the API calls are the go-github library's,
// and the installation tokens are ghinstallation's.
package github
