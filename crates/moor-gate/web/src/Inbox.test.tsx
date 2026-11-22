import { expect, test } from 'bun:test';

import type { BoxRow, InboxItem } from './api';
import { InboxView, inboxOrder } from './Inbox';
import { all, textOf } from './testkit';

const NOW = new Date(2026, 10, 22, 15, 0).getTime();
const at = (h: number, m: number) => new Date(2026, 10, 22, h, m).toISOString();

const item = (id: number, state: InboxItem['state'], over: Partial<InboxItem> = {}): InboxItem => ({ id, box: `web/${id}`, repo: 'acme/web', kind: 'needs_input', state, title: `Task ${id}`, text: `Asks ${id}`, at: at(14, id), ...over });

const items = [item(5, 'read'), item(4, 'unread'), item(3, 'resolved'), item(2, 'unread'), item(1, 'read')];
const view = (over: Partial<Parameters<typeof InboxView>[0]> = {}) => InboxView({ items, rows: [], now: NOW, unread: 2, onReadAll: () => {}, onOpen: () => {}, ...over });

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

test('it offers Mark all read while something is unread, and no way to remove an item', () => {
	const text = textOf(view());
	expect(text).toContain('Mark all read');
	expect(text).not.toMatch(/Clear|Remove/);
	expect(all(view(), 'button').map(textOf)).toEqual(['Mark all read']);
	expect(textOf(view({ unread: 0 }))).toContain('All read');
	expect(textOf(view({ items: [], unread: 0 }))).toContain('Nothing has needed you yet.');
});

test('opening an item reports which one', () => {
	const opened: number[] = [];
	all(view({ onOpen: (i) => opened.push(i.id) }), 'a')[1]?.props.onClick?.();
	expect(opened).toEqual([2]);
});
