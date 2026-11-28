// Package link is the runner link: the frames both sides exchange and the handshake that
// authenticates a runner to the portal. The fence that orders launches of one task is in wire. It
// owns no routing, no
// presence storage and no key storage; the portal supplies those through KeyStore and Presence.
package link
