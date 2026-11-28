package settingstest_test

import (
	"testing"

	"github.com/IniZio/moor/internal/domain/storetest"
	"github.com/IniZio/moor/internal/settings/settingstest"
)

func TestMemoryPassesTheBlockedContract(t *testing.T) {
	storetest.RunBlocked(t, func(*testing.T) storetest.BlockedStore { return settingstest.NewMemory() })
}

func TestMemoryPassesTheSettingsContract(t *testing.T) {
	storetest.RunSettings(t, func(*testing.T) storetest.SettingsStore { return settingstest.NewMemory() })
}
