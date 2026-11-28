package link

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"

	"github.com/IniZio/moor/internal/wire"
)

// maxFrame bounds one JSON frame. Bulk data is a byte stream, never a frame.
const maxFrame = 1 << 20

var errFrameTooBig = errors.New("link: frame too big")

// Request is frame 1 of every stream, written by the opener.
type Request struct {
	Op   wire.Kind       `json:"op"`
	Body json.RawMessage `json:"body,omitempty"`
}

// Reply is frame 2, written by the side that serves the op. After an Err the stream is closed.
type Reply struct {
	Err  *wire.Error     `json:"err,omitempty"`
	Body json.RawMessage `json:"body,omitempty"`
}

// WriteFrame writes a 4-byte big-endian length and the JSON of v in one Write, so a frame is never
// interleaved or split on the session.
func WriteFrame(w io.Writer, v any) error {
	payload, err := json.Marshal(v)
	if err != nil {
		return fmt.Errorf("link: encode frame: %w", err)
	}
	if len(payload) > maxFrame {
		return errFrameTooBig
	}
	buf := make([]byte, 4, 4+len(payload))
	binary.BigEndian.PutUint32(buf, uint32(len(payload)))
	if _, err := w.Write(append(buf, payload...)); err != nil {
		return fmt.Errorf("link: write frame: %w", err)
	}
	return nil
}

// ReadFrame reads exactly one frame and not a byte more, so the raw bytes that follow on a stream
// op are still in r. strict rejects unknown fields: requests are strict (a field the peer ignores
// could silently weaken an op), replies are lenient.
func ReadFrame(r io.Reader, v any, strict bool) error {
	var hdr [4]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		return fmt.Errorf("link: read frame header: %w", err)
	}
	n := binary.BigEndian.Uint32(hdr[:])
	if n > maxFrame {
		return errFrameTooBig
	}
	payload := make([]byte, n)
	if _, err := io.ReadFull(r, payload); err != nil {
		return fmt.Errorf("link: read frame: %w", err)
	}
	dec := json.NewDecoder(bytes.NewReader(payload))
	if strict {
		dec.DisallowUnknownFields()
	}
	if err := dec.Decode(v); err != nil {
		return fmt.Errorf("link: decode frame: %w", err)
	}
	return nil
}
