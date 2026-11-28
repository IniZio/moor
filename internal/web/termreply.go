package web

import "regexp"

// replyRE matches a message made only of what a terminal answers to the program it shows, never what a person
// types: colour and other OSC replies, DCS replies, device attribute, status and cursor reports, mode and keyboard
// flag reports, and focus in or out.
var replyRE = regexp.MustCompile("^(?:" + `\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)` + "|" + `\x1bP[^\x1b]*\x1b\\` + "|" +
	`\x1b\[[?>]?[0-9;]*[cnR]` + "|" + `\x1b\[\?[0-9;]*\$y` + "|" + `\x1b\[\?[0-9]*u` + "|" + `\x1b\[[IO]` + ")+$")

// terminalReply is true when a browser's terminal sent b by itself, in answer to the session's own queries. The
// session's terminal on the desk already answers those, so a reply from a viewer is neither typing nor input.
func terminalReply(b []byte) bool { return replyRE.Match(b) }
