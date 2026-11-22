import { expect, test } from 'bun:test';

import type { BoxRow } from './api';
import { menuItems, type MenuProps, RepoMenuView } from './RepoMenu';
import { recentRepos, usedTogether } from './repoSets';
import { all, type El, textOf } from './testkit';

const at = (d: number) => new Date(2026, 10, d, 9, 0).toISOString();

const row = (box: string, repos: string[], day: number): BoxRow => ({
	box,
	repo: repos[0] ?? null,
	other_repos: repos.slice(1),
	title: box,
	state: 'done',
	stale: false,
	line: '',
	last_event_at: at(day),
	updated_at: at(day),
	pr: null,
	failed: false,
	retryable: false,
});

const pair = ['acme/shop-api', 'acme/shop-web'];
const rows = [
	...Array.from({ length: 12 }, (_, i) => row(`p${i}`, pair, 10)),
	row('s', ['acme/docs'], 20),
	row('t', ['acme/docs', 'acme/shop-api', 'acme/tools'], 5),
	row('u', ['acme/a', 'acme/b'], 4),
	row('v', ['acme/c', 'acme/d'], 3),
];

type Calls = { change: string[][]; find: string[]; active: number[]; done: number };

function menu(over: Partial<MenuProps> = {}) {
	const calls: Calls = { change: [], find: [], active: [], done: 0 };
	const props: MenuProps = {
		repos: [],
		listed: recentRepos(rows, []).map((repo) => ({ repo, used: '' })),
		together: usedTogether(rows),
		find: '',
		active: -1,
		onFind: (t) => calls.find.push(t),
		onActive: (n) => calls.active.push(n),
		onChange: (r) => calls.change.push(r),
		onDone: () => {
			calls.done++;
		},
		...over,
	};
	return { view: RepoMenuView(props), calls, props };
}

const buttons = (view: ReturnType<typeof RepoMenuView>) => all(view, 'button');
const named = (view: ReturnType<typeof RepoMenuView>, label: string) => buttons(view).filter((b) => textOf(b).includes(label));

const press = (el: El | undefined, key: string) => el?.props.onKeyDown?.({ key, preventDefault: () => undefined, stopPropagation: () => undefined });

test('recent repos come most recently used first', () => {
	expect(recentRepos(rows, ['acme/zeta']).slice(0, 3)).toEqual(['acme/docs', 'acme/shop-api', 'acme/shop-web']);
	expect(recentRepos(rows, ['acme/zeta']).at(-1)).toBe('acme/zeta');
});

test('used together lists up to three sets of two or more repos, the most used first', () => {
	const sets = usedTogether(rows);
	expect(sets).toHaveLength(3);
	expect(sets[0]).toEqual({ repos: pair, count: 12 });
	expect(sets.every((s) => s.repos.length >= 2)).toBe(true);
});

test('a used-together line shows its count and sets both repos at once', () => {
	const { view, calls } = menu({ repos: ['acme/docs'] });
	const [line] = named(view, 'acme/shop-api + acme/shop-web');
	expect(textOf(line)).toContain('12 tasks');
	line.props.onClick?.();
	expect(calls.change).toEqual([pair]);
});

test('checking two repos keeps both, in the order checked', () => {
	const first = menu();
	named(first.view, 'acme/shop-web').find((b) => b.props.role === 'menuitemcheckbox')?.props.onClick?.();
	expect(first.calls.change).toEqual([['acme/shop-web']]);
	const second = menu({ repos: ['acme/shop-web'] });
	const row = named(second.view, 'acme/docs').find((b) => b.props.role === 'menuitemcheckbox');
	row?.props.onClick?.();
	expect(second.calls.change).toEqual([['acme/shop-web', 'acme/docs']]);
	const checked = buttons(second.view).filter((b) => b.props['aria-checked'] === true);
	expect(checked.map(textOf).join(' ')).toContain('acme/shop-web');
});

test('the first repo is marked, another can start here, and a repo can be dropped', () => {
	const repos = ['acme/shop-api', 'acme/shop-web', 'acme/docs'];
	const { view, calls } = menu({ repos });
	expect(textOf(view).match(/starts here/g)).toHaveLength(1);
	const start = named(view, 'start here');
	expect(start).toHaveLength(2);
	start[1].props.onClick?.();
	const drops = buttons(view).filter((b) => textOf(b) === '×');
	drops[1].props.onClick?.();
	expect(calls.change).toEqual([
		['acme/docs', 'acme/shop-api', 'acme/shop-web'],
		['acme/shop-api', 'acme/docs'],
	]);
});

test('Enter on a full owner/name that is not listed adds it, and otherwise closes the menu', () => {
	const typed = menu({ repos: ['acme/shop-api'], find: 'acme/brand-new' });
	const input = all(typed.view, 'input')[0];
	expect(menuItems(typed.props).map((i) => i.kind)).toEqual(['add']);
	press(input, 'Enter');
	expect(typed.calls.change).toEqual([['acme/shop-api', 'acme/brand-new']]);
	expect(typed.calls.find).toEqual(['']);
	expect(typed.calls.done).toBe(0);

	const plain = menu({ find: 'docs' });
	press(all(plain.view, 'input')[0], 'Enter');
	expect(plain.calls.done).toBe(1);
	expect(plain.calls.change).toEqual([]);
});

test('arrows move, Space checks the row under them and Escape closes', () => {
	const { view, calls, props } = menu({ active: 0 });
	const input = all(view, 'input')[0];
	press(input, 'ArrowDown');
	expect(calls.active).toEqual([1]);
	press(input, 'Space');
	press(input, ' ');
	expect(calls.change).toEqual([pair]);
	const idle = menu({ active: -1 });
	press(all(idle.view, 'input')[0], ' ');
	expect(idle.calls.change).toEqual([]);
	expect(props.active).toBe(0);
	press(view as El, 'Escape');
	expect(calls.done).toBe(1);
});
