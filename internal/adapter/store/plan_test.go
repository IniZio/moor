package store

import (
	"context"
	"strings"
	"testing"
)

// planOf runs EXPLAIN for a query with its arguments and returns the plan text.
func planOf(t *testing.T, s *Store, query string, args ...any) string {
	t.Helper()
	rows, err := s.pool.Query(context.Background(), "EXPLAIN (COSTS OFF) "+query, args...)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var lines []string
	for rows.Next() {
		var line string
		if err := rows.Scan(&line); err != nil {
			t.Fatal(err)
		}
		lines = append(lines, line)
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	return strings.Join(lines, "\n")
}

func TestKeyedReadsUseTheKeyIndex(t *testing.T) {
	s := freshStore(t)
	ctx := context.Background()
	_, err := s.pool.Exec(ctx, `INSERT INTO tasks (id, repo_host, repo_path, prompt, state, version, created_at, spec_name, spec_file, event_source, event_group, event_rev)
		SELECT 't' || i, 'h', 'p' || (i % 10), '', CASE WHEN i % 3 = 0 THEN 'running' ELSE 'done' END, 1, now(), 'review', 'specs/review.yaml', 'forge', 'g' || (i / 10), 'r' || i
		FROM generate_series(1, 20000) AS i`)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.pool.Exec(ctx, "ANALYZE tasks"); err != nil {
		t.Fatal(err)
	}
	plans := map[string]string{
		"ByKey":       planOf(t, s, selectByKey, "h", "p3", "review", "forge", "g3", "r33"),
		"LiveInGroup": planOf(t, s, selectLive, "h", "p3", "review", "forge", "g3", liveStates()),
	}
	for name, plan := range plans {
		t.Logf("%s plan:\n%s", name, plan)
		if strings.Contains(plan, "Seq Scan") || !strings.Contains(plan, "tasks_event_key") {
			t.Errorf("%s does not use tasks_event_key:\n%s", name, plan)
		}
	}
}
