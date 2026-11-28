package main

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"strings"
	"time"
)

type joinOptions struct{ portal, id, token, keyFile, name string }

func parseJoin(args []string) (joinOptions, error) {
	var o joinOptions
	host, _ := os.Hostname()
	fs := flag.NewFlagSet("runner join", flag.ContinueOnError)
	fs.StringVar(&o.portal, "portal", "", "the portal's address, such as https://portal.example.com")
	fs.StringVar(&o.id, "id", "", "the machine id the portal showed")
	fs.StringVar(&o.token, "token", "", "the one-time join token the portal showed")
	fs.StringVar(&o.keyFile, "key", "", "file to keep this runner's ed25519 seed in; an existing one is used again")
	fs.StringVar(&o.name, "name", host, "the machine's name in the portal")
	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("flags: %w", err)
	}
	if o.portal == "" || o.id == "" || o.token == "" || o.keyFile == "" {
		return o, errors.New("join needs -portal, -id, -token and -key")
	}
	return o, nil
}

// join enrols this host with the portal: it makes the runner's key, or reuses the one in the key file,
// and sends the public half with the one-time token.
func join(args []string) error {
	o, err := parseJoin(args)
	if err != nil {
		return err
	}
	key, err := keyFor(o.keyFile)
	if err != nil {
		return err
	}
	body, err := json.Marshal(map[string]any{"id": o.id, "token": o.token, "name": o.name, "public_key": []byte(key.Public().(ed25519.PublicKey))})
	if err != nil {
		return fmt.Errorf("join: %w", err)
	}
	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Post(strings.TrimSuffix(o.portal, "/")+"/runner/join", "application/json", bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("join: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		msg, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<10))
		return fmt.Errorf("join refused (%s): %s", resp.Status, strings.TrimSpace(string(msg)))
	}
	fmt.Printf("joined as %s; the key is in %s\n", o.id, o.keyFile)
	return nil
}

// keyFor reads the key in path, or makes one and writes it there, readable by this user alone. A
// join run again after a failure keeps the key it already made.
func keyFor(path string) (ed25519.PrivateKey, error) {
	key, err := readKey(path)
	if err == nil || !errors.Is(err, fs.ErrNotExist) {
		return key, err
	}
	seed := make([]byte, ed25519.SeedSize)
	if _, err := rand.Read(seed); err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	_, err = f.WriteString(base64.StdEncoding.EncodeToString(seed) + "\n")
	if err = errors.Join(err, f.Close()); err != nil {
		return nil, fmt.Errorf("key: %w", err)
	}
	return ed25519.NewKeyFromSeed(seed), nil
}
