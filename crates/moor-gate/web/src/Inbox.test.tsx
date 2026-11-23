import { expect, test } from 'bun:test';

import type { BoxRow, InboxItem } from './api';
import { InboxView, inboxOrder } from './Inbox';
import { all, textOf } from './testkit';

const NOW = new Date(2026, 10, 22, 15, 0).getTime();
const at = (h: number, m: number) => new Date(2026, 10, 22, h, m).toISOString();

const item = (id: number, state: InboxItem['state'], over: Partial<InboxItem> = {}): InboxItem => ({ id, box: `web/${id}`, repo: 'acme/web', kind: 'needs_input', state, title: `Task ${id}`, text: `Asks ${id}`, at: at(14, id), ...over });

const items = [item(5, 'read'), item(4, 'unread'), item(3, 'resolved'), item(2, 'unread'), item(1, 'read')];
const view = (over: Partial<Parameters<typeof InboxView>[0]> = {}) => InboxView({ items, rows: [], now: NOW, unread: 2, onReadAll: () => {}, onOpen: () => {}, onRemove: () => {}, onClearAll: () => {}, ...over });

test('unread items come first, each part newest first as sent', () => {
	expect(inboxOrder(items).map((i) => i.id)).toEqual([4, 2, 5, 3, 1]);
});

test('the dropdown lists the items in that order, each with its repo, title, line and exact time', () => {
	const rows = all(view(), 'a');
	expect(rows.map((a) => textOf(a).match(/Task \d/)?.[0])).toEqual(['Task 4', 'Task 2', 'Task 5', 'Task 3', 'Task 1']);
	expect(textOf(rows[0])).toContain('acme/web');
	expect(textOf(rows[0])).toContain('Asks 4');
	expect(textOf(rows[0])).toContain('14:04');
	expect(textOf(rows[3])).toContain('Answered: Asks 3');
	expect(rows[0]?.props.href).toBe('#/box/web%2F4');
});

test('an item lists every repo of its task when the task is known', () => {
	const task = { box: 'web/4', repo: 'acme/web', other_repos: ['acme/api'] } as BoxRow;
	expect(textOf(all(view({ rows: [task] }), 'a')[0])).toContain('+1');
});

test('it offers Mark all read while something is unread, and Clear all with a note while it lists anything', () => {
	const text = textOf(view());
	expect(text).toContain('Mark all read');
	expect(text).toContain('Removing an item does not change its task.');
	const heads = all(view(), 'button').filter((b) => !b.props['aria-label']);
	expect(heads.map(textOf)).toEqual(['Mark all read', 'Clear all']);
	expect(textOf(view({ unread: 0 }))).toContain('All read');
	const empty = textOf(view({ items: [], unread: 0 }));
	expect(empty).toContain('Nothing new.');
	expect(empty).not.toMatch(/Clear all|Removing/);
});

test('Clear all clears through the newest item it lists, so one that lands later stays', () => {
	const through: number[] = [];
	const clearAll = all(view({ onClearAll: (n) => through.push(n) }), 'button').find((b) => textOf(b) === 'Clear all');
	clearAll?.props.onClick?.();
	expect(through).toEqual([5]);
});

test('each item has a Remove button, and Delete or Backspace on a focused item removes it', () => {
	const removed: [number, number][] = [];
	const tree = view({ onRemove: (i, at) => removed.push([i.id, at]) });
	const buttons = all(tree, 'button').filter((b) => b.props['aria-label'] === 'Remove');
	expect(buttons.map(textOf)).toEqual(['×', '×', '×', '×', '×']);
	buttons[1]?.props.onClick?.();
	const rows = all(tree, 'div').filter((d) => d.props.onKeyDown);
	let stopped = 0;
	const key = (k: string) => ({ key: k, preventDefault: () => stopped++, stopPropagation: () => {} });
	rows[2]?.props.onKeyDown?.(key('Delete'));
	rows[0]?.props.onKeyDown?.(key('Backspace'));
	rows[0]?.props.onKeyDown?.(key('Enter'));
	expect(removed).toEqual([
		[2, 1],
		[5, 2],
		[4, 0],
	]);
	expect(stopped).toBe(2);
});

test('an item whose task waits at a gate says what the Tasks home says of it', () => {
	const task = { box: 'web/4', repo: 'acme/web', other_repos: [], state: 'needs_you', gate: { gates: ['review'], status: { state: 'open', gate: 'review', try_n: 1, next: 'ask' } } } as unknown as BoxRow;
	expect(textOf(all(view({ rows: [task] }), 'a')[0])).toContain('Waiting for your yes on “review”');
});

test('opening an item reports which one', () => {
	const opened: number[] = [];
	all(view({ onOpen: (i) => opened.push(i.id) }), 'a')[1]?.props.onClick?.();
	expect(opened).toEqual([2]);
});
