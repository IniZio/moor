package domain

import (
	"errors"
	"fmt"
	"time"
)

// Credential is one named secret the proxies add on the wire, or the GitHub App the product acts as. Exactly
// one of Value and App is set.
type Credential struct {
	Name  string
	Value SecretState
	App   *GitHubApp
}

// GitHubApp is a GitHub App credential. The product mints a token from its private key per task, limited to
// the task's repository. APIURL is GitHub's API address, https://api.github.com/ when empty.
type GitHubApp struct {
	AppID         int64
	PrivateKey    SecretState
	WebhookSecret SecretState
	APIURL        string
}

// The fields of a credential that hold a secret.
const (
	FieldValue         = "value"
	FieldPrivateKey    = "private_key"
	FieldWebhookSecret = "webhook_secret"
)

// SecretField is one secret field of a credential and what the document says of it.
type SecretField struct {
	Field string
	State SecretState
}

// SecretKey is the key a secret is stored under: the credential's name and the field.
func SecretKey(credential, field string) string { return credential + "/" + field }

// Fields lists the credential's secret fields.
func (c Credential) Fields() []SecretField {
	if c.App != nil {
		return []SecretField{{FieldPrivateKey, c.App.PrivateKey}, {FieldWebhookSecret, c.App.WebhookSecret}}
	}
	return []SecretField{{FieldValue, c.Value}}
}

// Set is true when every secret the credential needs to add a value is set. A GitHub App needs its private key.
func (c Credential) Set() bool {
	if c.App != nil {
		return c.App.PrivateKey == SecretSet
	}
	return c.Value == SecretSet
}

func (c Credential) validate() error {
	if _, err := ParseName(c.Name); err != nil {
		return err
	}
	if (c.App == nil) == (c.Value == "") {
		return errors.New("a credential has a value or a github_app, not both")
	}
	for _, f := range c.Fields() {
		if f.State != SecretSet && f.State != SecretNotSet {
			return fmt.Errorf("%s has an unknown state", f.Field)
		}
	}
	if c.App != nil && c.App.AppID <= 0 {
		return errors.New("app_id must be a positive number")
	}
	return nil
}

// CredentialValue is what the proxy adds for a credential: a stored value, or a token minted for the task,
// which expires.
type CredentialValue struct {
	Value   string
	Expires time.Time
}

// AppKeys is a GitHub App credential with its secrets open, for the code that acts as the App.
type AppKeys struct {
	AppID         int64
	PrivateKey    []byte
	WebhookSecret []byte
	APIURL        string
}

// GitHubAPI is GitHub's API address when the credential names none.
const GitHubAPI = "https://api.github.com/"

// API is the App's API address.
func (k AppKeys) API() string {
	if k.APIURL == "" {
		return GitHubAPI
	}
	return k.APIURL
}
