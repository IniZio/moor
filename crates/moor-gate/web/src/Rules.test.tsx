import { expect, test } from 'bun:test';

import type { RepoConfig } from './api';
import { lineNumbers, rulesLine } from './Rules';

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
