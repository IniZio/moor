package domain

import (
	"errors"
	"time"
)

// ErrStaleSettings is a settings save whose base is not the newest version, because someone saved first.
var ErrStaleSettings = errors.New("stale settings version")

// SettingsVersion is one saved version of the org settings document. N counts from 1.
type SettingsVersion struct {
	N       int
	Text    string
	Note    string
	SavedBy string
	SavedAt time.Time
}

// SealedSecret is a connection's secret value, encrypted. KeyID names the key that sealed it.
type SealedSecret struct {
	Connection string
	Ciphertext []byte
	KeyID      string
	UpdatedAt  time.Time
}

// SettingsWrite is one atomic save: the document as version Base+1 and the secrets it carried.
//
// Base is the version the writer loaded, zero when none exists. The store refuses any other base with
// ErrStaleSettings. Keep names the connections in Text. A stored secret of any other connection is dropped,
// so a deleted connection leaves no usable value. Secrets of kept connections not listed in Secrets stay.
type SettingsWrite struct {
	Base    int
	Text    string
	Note    string
	By      string
	At      time.Time
	Keep    []string
	Secrets []SealedSecret
}
