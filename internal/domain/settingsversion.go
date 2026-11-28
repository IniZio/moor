package domain

import (
	"errors"
	"fmt"
	"strings"
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

// SealedSecret is a credential's secret, encrypted, under its key from SecretKey. KeyID names the key that sealed it.
type SealedSecret struct {
	Key        string
	Ciphertext []byte
	KeyID      string
	UpdatedAt  time.Time
}

// SettingsWrite is one atomic save: the document as version Base+1 and the secrets it carried.
//
// Base is the version the writer loaded, zero when none exists. The store refuses any other base with
// ErrStaleSettings. Keep names the secrets Text says are set. Any other stored secret is dropped, so a deleted
// credential, or one set to not set, leaves no usable value. Kept secrets not listed in Secrets stay.
type SettingsWrite struct {
	Base    int
	Text    string
	Note    string
	By      string
	At      time.Time
	Keep    []string
	Secrets []SealedSecret
}

// SettingsProblem is one thing wrong in a typed document. Line counts from 1 and is 0 when the problem has no line.
type SettingsProblem struct {
	Line    int
	Message string
}

// InvalidSettingsError lists every problem of a refused document.
type InvalidSettingsError struct{ Problems []SettingsProblem }

func (e *InvalidSettingsError) Error() string {
	parts := make([]string, len(e.Problems))
	for i, p := range e.Problems {
		parts[i] = fmt.Sprintf("line %d: %s", p.Line, p.Message)
	}
	return "invalid settings: " + strings.Join(parts, "; ")
}

// StaleSettingsError is a save based on a version that is no longer the newest.
type StaleSettingsError struct{ Newest int }

func (e *StaleSettingsError) Error() string {
	return fmt.Sprintf("settings changed: the newest version is v%d", e.Newest)
}

func (e *StaleSettingsError) Is(target error) bool { return target == ErrStaleSettings }

// SettingsSave is a whole document typed by a person, based on version Base.
type SettingsSave struct {
	Base int
	Text string
	Note string
	By   string
}

// SettingsSaved is what a save did. Unchanged means the document and its values matched the newest version,
// so no version was made. Replaced names the secrets, credential/field, whose value was typed.
type SettingsSaved struct {
	Version   SettingsVersion
	Unchanged bool
	Replaced  []string
}

type DiffKind uint8

const (
	DiffSame DiffKind = iota
	DiffAdded
	DiffRemoved
)

type DiffLine struct {
	Kind DiffKind
	Text string
}

// SettingsEntry is a saved version with its diff against the version before it. Version 1 diffs against nothing.
type SettingsEntry struct {
	SettingsVersion
	Diff []DiffLine
}
