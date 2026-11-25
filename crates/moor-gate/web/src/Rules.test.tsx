import { expect, test } from 'bun:test';

import type { RepoConfig } from './api';
import { all, textOf } from './testkit';
import { afterSave, hostPort, lineNumbers, rulesLine, Seen } from './Rules';

const rules = (over: Partial<RepoConfig> = {}): RepoConfig => ({
	repo: 'acme/web',
	text: 'rules:\n  - hosts: [api.github.com]\n',
	revision: 0,
	edited_by: null,
	edited_at: null,
	can_edit: true,
	...over,
});

test('a repo with no rules of its own says it follows the default rules', () => {
	expect(rulesLine(rules())).toBe('Shared by everyone who works on acme/web · the default rules');
});

test('saved rules name their revision and who last edited them', () => {
	const line = rulesLine(rules({ revision: 12, edited_by: 'alice', edited_at: '2026-11-18T10:00:00Z' }));
	expect(line.startsWith('Shared by everyone who works on acme/web · revision 12 · edited by alice ')).toBe(true);
	expect(line).toContain('18');
});

test('each line of the text gets one number, and a closing newline adds none', () => {
	expect(lineNumbers('rules:\n  - hosts: [a]\n')).toBe('1\n2');
	expect(lineNumbers('rules:\n  - hosts: [a]')).toBe('1\n2');
	expect(lineNumbers('')).toBe('1');
});

test('a host shows its port only when it is not 443', () => {
	expect(hostPort('registry.npmjs.org', 443)).toBe('registry.npmjs.org');
	expect(hostPort('151.101.1.1', 80)).toBe('151.101.1.1:80');
});

test('the lists show each withheld request with its host and Boxes, and each dialled host with its count', () => {
	const view = Seen({
		seen: {
			days: 7,
			withheld: [{ request: 'GET /user', host: 'api.github.com', boxes: 2 }],
			dialled: [{ host: 'registry.npmjs.org', port: 443, count: 212 }],
		},
	});
	const rows = all(view, 'li').map((li) => textOf(li));
	expect(rows).toEqual(['GET /userapi.github.com2', 'registry.npmjs.org212']);
	expect(textOf(view)).toContain('Withheld in the last 7 days');
});

test('a save keeps the lists the page read, since the write reply has none', () => {
	const seen = { days: 7, withheld: [{ request: 'GET /user', host: 'api.github.com', boxes: 1 }], dialled: [] };
	const saved = afterSave(rules({ seen }), rules({ revision: 2, seen: null }));
	expect(saved.revision).toBe(2);
	expect(saved.seen).toEqual(seen);
});
