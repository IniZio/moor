import { expect, test } from 'bun:test';

import type { BoxRow } from './api';
import { exactTime, HomeListView } from './HomeList';
import { all, textOf } from './testkit';

const NOW = new Date(2026, 10, 22, 15, 0).getTime();
const at = (y: number, mo: number, d: number, h: number, mi: number) => new Date(y, mo - 1, d, h, mi).toISOString();

const row = (box: string, repo: string | null, over: Partial<BoxRow> = {}): BoxRow => ({
	box,
	repo,
	other_repos: [],
	title: `Task ${box}`,
	state: 'working',
	stale: false,
	line: 'Running the tests',
	last_event_at: at(2026, 11, 22, 14, 46),
	updated_at: at(2026, 11, 22, 14, 46),
	pr: null,
	failed: false,
	retryable: false,
	...over,
});

const rows = [
	row('a', 'acme/web', { other_repos: ['acme/api'], state: 'needs_you' }),
	row('b', 'acme/api', { pr: { repo: 'acme/api', number: 42, url: 'https://example.test/pr/42' }, state: 'done' }),
	row('c', 'acme/docs', { state: 'done', last_event_at: at(2026, 10, 1, 9, 0) }),
	row('d', 'acme/web', { state: 'closed' }),
];

const rowsOf = (view: ReturnType<typeof HomeListView>) => all(view, 'a').filter((a) => 'data-row' in a.props);
const titles = (view: ReturnType<typeof HomeListView>) => all(view, 'span').map((s) => (s.props as { title?: string }).title);

test('a time is the clock today, the day and clock this year, and the year as well before that', () => {
	expect(exactTime(at(2026, 11, 22, 14, 46), NOW)).toBe('14:46');
	expect(exactTime(at(2026, 11, 21, 14, 5), NOW)).toBe('21 Nov 14:05');
	expect(exactTime(at(2025, 11, 3, 9, 7), NOW)).toBe('3 Nov 2025 09:07');
	expect(exactTime('not a time', NOW)).toBe('');
});

test('a row shows its exact time and a pull request only when it has one', () => {
	const view = HomeListView({ rows, now: NOW, hash: '#/' });
	const text = rowsOf(view).map(textOf);
	expect(text[0]).toContain('14:46');
	expect(text[0]).not.toContain('PR #');
	expect(text.find((t) => t.includes('Task b'))).toContain('PR #42');
	expect(textOf(view)).not.toMatch(/\d+[mhd]\b/);
});

test('a row whose line already names its pull request does not name it again beside the title', () => {
	const done = row('e', 'acme/web', { state: 'done', line: 'fixed · PR #13', pr: { repo: 'acme/web', number: 13, url: 'https://example.test/pr/13' } });
	const text = textOf(rowsOf(HomeListView({ rows: [done], now: NOW, hash: '#/' }))[0]);
	expect(text.match(/PR #13/g)).toHaveLength(1);
});

test('a task on two repos shows the first with +1 and names both on hover', () => {
	const first = rowsOf(HomeListView({ rows, now: NOW, hash: '#/' }))[0];
	expect(textOf(first)).toContain('acme/web');
	expect(textOf(first)).toContain('+1');
	expect(titles(first as never)).toContain('acme/web, acme/api');
});

test('filtering by a repo that is second in a task\'s list still lists the task, and its cell shows only the others', () => {
	const view = HomeListView({ rows, now: NOW, hash: '#/?repo=acme/api' });
	const shown = rowsOf(view);
	expect(shown.map((a) => textOf(a).includes('Task a') || textOf(a).includes('Task b'))).toEqual([true, true]);
	expect(textOf(view)).toContain('acme/api');
	const a = shown.find((s) => textOf(s).includes('Task a'));
	expect(textOf(a)).toContain('+ acme/web');
	expect(textOf(a)).not.toContain('+1');
	const b = shown.find((s) => textOf(s).includes('Task b'));
	expect(textOf(b)).not.toContain('acme/api +');
});

test('a repo nobody has tasks on is ignored', () => {
	expect(rowsOf(HomeListView({ rows, now: NOW, hash: '#/?repo=nope/none' }))).toHaveLength(rows.length);
});

test('grouping by repo keeps every row, each under its first repo', () => {
	const byState = rowsOf(HomeListView({ rows, now: NOW, hash: '#/' }));
	const byRepo = HomeListView({ rows, now: NOW, hash: '#/?grp=repo' });
	expect(rowsOf(byRepo)).toHaveLength(byState.length);
	const heads = all(byRepo, 'h2').map((h) => textOf(h).trim());
	expect(heads).toEqual(['acme/api · 1', 'acme/web · 1']);
});

test('the filter line offers the repos that have tasks and keeps the grouping when a repo is picked', () => {
	const view = HomeListView({ rows, now: NOW, hash: '#/?grp=repo' });
	const links = all(view, 'a').map((a) => [textOf(a), a.props.href]);
	expect(links).toContainEqual(['acme/api', '#/?grp=repo&repo=acme/api']);
	expect(links).toContainEqual(['All repos', '#/?grp=repo']);
	expect(links).toContainEqual(['state', '#/']);
});

const theirs = [
	row('mine', 'acme/web', { state: 'needs_you' }),
	row('ana', 'acme/web', { state: 'needs_you', owner: 'ana@example.com' }),
	row('bo', 'acme/api', { state: 'done', owner: 'bo@example.com' }),
];

test('the filter offers my tasks and everyone\'s, and keeps the choice in the address', () => {
	const links = (hash: string) => all(HomeListView({ rows: theirs, now: NOW, hash }), 'a').filter((a) => !('data-row' in a.props));
	const mine = links('#/').find((a) => textOf(a) === 'Mine');
	const everyone = links('#/').find((a) => textOf(a) === 'Everyone');
	expect(mine?.props.href).toBe('#/');
	expect(everyone?.props.href).toBe('#/?who=everyone');
	expect(mine?.props['aria-current']).toBe('true');
	const on = links('#/?who=everyone').find((a) => textOf(a) === 'Everyone');
	expect(on?.props['aria-current']).toBe('true');
});

test('under Everyone another person\'s task names its owner, waits under Working, and offers no peek', () => {
	const view = HomeListView({ rows: theirs, now: NOW, hash: '#/?who=everyone' });
	const heads = all(view, 'h2').map((h) => textOf(h).trim());
	expect(heads).toContain('Needs you · 1');
	expect(heads).toContain('Working · 1');
	const ana = rowsOf(view).find((a) => textOf(a).includes('Task ana'));
	expect(textOf(ana)).toContain('ana@example.com');
	expect(ana?.props['data-peek']).toBeUndefined();
	const own = rowsOf(view).find((a) => textOf(a).includes('Task mine'));
	expect(textOf(own)).not.toContain('@');
});
