package web

import "testing"

func TestATerminalsOwnRepliesAreNoTyping(t *testing.T) {
	replies := []string{
		"\x1b]10;rgb:efef/eaea/e2e2\x1b\\", "\x1b]11;rgb:1717/1414/1111\x07", "\x1b[?1;2c", "\x1b[>0;276;0c", "\x1b[O", "\x1b[I",
		"\x1b[12;40R", "\x1b[0n", "\x1b[?2004;2$y", "\x1b[?0u", "\x1bP>|xterm.js(6.0.0)\x1b\\", "\x1b[?1;2c\x1b[O\x1b[O",
	}
	for _, r := range replies {
		if !terminalReply([]byte(r)) {
			t.Errorf("%q is a terminal's reply, read as typing", r)
		}
	}
	keys := []string{"", "a", "ls\r", "\x03", "\x1b", "\x1b[A", "\x1bOA", "\x1b[15~", "\x1b[1;5C", "\x1b[?1;2cls\r", "\x1b[200~text\x1b[201~"}
	for _, k := range keys {
		if terminalReply([]byte(k)) {
			t.Errorf("%q is typing, read as a terminal's reply", k)
		}
	}
}
