package tasktest_test

import (
	"context"
	"testing"

	"github.com/IniZio/moor/internal/domain"
	"github.com/IniZio/moor/internal/domain/storetest"
	"github.com/IniZio/moor/internal/task/tasktest"
)

func TestMemoryPassesTheStoreContract(t *testing.T) {
	storetest.Run(t, func(*testing.T) storetest.Subject {
		m := tasktest.NewMemory()
		return storetest.Subject{
			Store: m,
			Events: func(_ context.Context, id domain.TaskID) ([]domain.EventRow, error) {
				return m.Events(id), nil
			},
			Page: m.Page,
		}
	})
}
