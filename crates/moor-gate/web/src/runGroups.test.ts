import { expect, test } from 'bun:test';

import type { RunRow } from './api';
import { groupByPull, shortSha } from './runGroups';

const row = (id: number, pull: string | null): RunRow => ({ id, pull, due_at: id, trigger: pull ? 'pull_request' : 'cron', status: 'success', label: 'finished' }) as RunRow;

test('runs group by pull request, each group where its newest run stands', () => {
	const groups = groupByPull([row(5, 'acme/web#482'), row(4, 'acme/api#96'), row(3, 'acme/web#482'), row(2, null), row(1, 'acme/web#479')]);
	expect(groups.map((g) => [g.pull, g.runs.map((r) => r.id)])).toEqual([
		['acme/web#482', [5, 3]],
		['acme/api#96', [4]],
		[null, [2]],
		['acme/web#479', [1]],
	]);
});

test('runs for no pull request stay one group', () => {
	expect(groupByPull([row(2, null), row(1, null)])).toEqual([{ pull: null, runs: [row(2, null), row(1, null)] }]);
	expect(groupByPull([])).toEqual([]);
});

test('a head commit shows its first seven characters', () => {
	expect(shortSha('a41c9e2f00d')).toBe('a41c9e2');
});
