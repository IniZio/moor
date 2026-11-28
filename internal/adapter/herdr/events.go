package herdr

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
)

type Event struct {
	Seq      uint64 `json:"seq"`
	Terminal string `json:"terminal"`
	Type     string `json:"type"`
}

// Subscribe turns a fresh stream into an event feed for the terminals carrying every label in
// labels. It calls snapshot once, then on for each event, until the stream or ctx ends. A subscribed
// connection answers nothing else, so the caller gives it a stream of its own. Subscribe closes rwc
// before it returns.
func Subscribe(ctx context.Context, rwc io.ReadWriteCloser, labels map[string]string, snapshot func([]Terminal), on func(Event)) error {
	defer func() { _ = rwc.Close() }()
	stop := context.AfterFunc(ctx, func() { _ = rwc.Close() })
	defer stop()
	req, err := json.Marshal(map[string]any{
		"jsonrpc": "2.0", "id": 1, "method": "events.subscribe", "params": map[string]any{"labels": labels},
	})
	if err != nil {
		return fmt.Errorf("hc: subscribe: %w", err)
	}
	if _, werr := rwc.Write(append(req, '\n')); werr != nil {
		err = fmt.Errorf("hc: subscribe: %w", werr)
	} else {
		err = feed(rwc, snapshot, on)
	}
	if ctx.Err() != nil {
		return fmt.Errorf("hc: subscribe: %w", ctx.Err())
	}
	return err
}

func feed(r io.Reader, snapshot func([]Terminal), on func(Event)) error {
	sc := newScanner(r)
	for sc.Scan() {
		var m message
		if json.Unmarshal(sc.Bytes(), &m) != nil {
			continue
		}
		if err := dispatch(m, snapshot, on); err != nil {
			return err
		}
	}
	if err := sc.Err(); err != nil {
		return fmt.Errorf("hc: subscribe: %w", err)
	}
	return fmt.Errorf("hc: subscribe: %w", ErrClosed)
}

func dispatch(m message, snapshot func([]Terminal), on func(Event)) error {
	switch {
	case m.Error != nil:
		return m.Error.err()
	case m.ID == 1:
		var res struct {
			Terminals []Terminal `json:"terminals"`
		}
		if err := json.Unmarshal(m.Result, &res); err != nil {
			return fmt.Errorf("hc: subscribe: %w", err)
		}
		snapshot(res.Terminals)
	case m.Method == "event":
		var ev Event
		if err := json.Unmarshal(m.Params, &ev); err != nil {
			return fmt.Errorf("hc: event: %w", err)
		}
		on(ev)
	}
	return nil
}
