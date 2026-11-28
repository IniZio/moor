package main

import (
	"flag"
	"fmt"

	"github.com/IniZio/moor/internal/adapter/store"
)

// bindStorage adds the flags that place the portal in a database it may share and give it settings at its first start.
func (c *config) bindStorage(fs *flag.FlagSet, getenv func(string) string) {
	fs.StringVar(&c.databaseSchema, "database-schema", getenv("PORTAL_DATABASE_SCHEMA"), "Postgres schema that holds every table and migration of the portal, made when missing (PORTAL_DATABASE_SCHEMA); without it the schema the database url selects")
	fs.StringVar(&c.seedSettingsFile, "seed-settings-file", getenv("PORTAL_SEED_SETTINGS_FILE"), "file with a whole settings document, saved as version 1 when no version exists (PORTAL_SEED_SETTINGS_FILE); it may hold real secret values")
}

func (c *config) checkStorage() error {
	if err := store.CheckSchema(c.databaseSchema); err != nil {
		return fmt.Errorf("database-schema: %w", err)
	}
	return nil
}
