package settingstest_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain/storetest"
	"github.com/IniZio/moor/internal/settings/settingstest"
)

func TestMemoryPassesTheSettingsContract(t *testing.T) {
	storetest.RunSettings(t, func(*testing.T) storetest.SettingsStore { return settingstest.NewMemory() })
}
