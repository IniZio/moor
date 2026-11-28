// Package link is the runner link: the frames both sides exchange, the handshake that authenticates
// a runner to the portal, and the fence that orders launches of one task. It owns no routing, no
// presence storage and no key storage; the portal supplies those through KeyStore and Presence.
package link
