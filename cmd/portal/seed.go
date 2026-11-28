package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"os"

	"github.com/IniZio/moor/internal/domain"
)

// seedAuthor is who History shows for the first version when a file made it.
const seedAuthor = "seed"

// settingsSaver is the part of the settings service the seed uses.
type settingsSaver interface {
	Current(ctx context.Context) (domain.SettingsVersion, error)
	Save(ctx context.Context, in domain.SettingsSave) (domain.SettingsSaved, error)
}

// seedSettings saves the document in path as version 1, the way a person's save would, when no version exists.
// Once any version exists it does nothing and does not read the file. A replica that loses the race to version 1
// finds the settings made and is done. An invalid document stops the start with the problems found, which never
// hold a secret value.
func seedSettings(ctx context.Context, svc settingsSaver, path string) error {
	if path == "" {
		return nil
	}
	cur, err := svc.Current(ctx)
	if err != nil {
		return fmt.Errorf("seed settings: %w", err)
	}
	if cur.N > 0 {
		return nil
	}
	text, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("seed settings: read %s: %w", path, err)
	}
	_, err = svc.Save(ctx, domain.SettingsSave{Base: 0, Text: string(text), By: seedAuthor})
	switch {
	case errors.Is(err, domain.ErrStaleSettings):
		return nil
	case err != nil:
		return fmt.Errorf("seed settings from %s: %w", path, err)
	}
	log.Printf("settings seeded from %s as version 1", path)
	return nil
}
