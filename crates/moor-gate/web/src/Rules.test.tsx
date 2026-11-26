import { expect, test } from 'bun:test';

import type { RepoConfig } from './api';
import { all, textOf } from './testkit';
import { afterSave, hostPort, lineNumbers, replyOf, RulesDiff, rulesAt, rulesLine, Seen, spans } from './Rules';

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
	expect(line.endsWith(' 18 Nov')).toBe(true);
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
	expect(all(view, 'button').length).toBe(1);
});

test('a save keeps the lists the page read, since the write reply has none', () => {
	const seen = { days: 7, withheld: [{ request: 'GET /user', host: 'api.github.com', boxes: 1 }], dialled: [] };
	const saved = afterSave(rules({ seen }), rules({ revision: 2, seen: null }));
	expect(saved.revision).toBe(2);
	expect(saved.seen).toEqual(seen);
});

test("the agent's quoted parts read as code", () => {
	const out = spans('I added `- "GET /user"` under `api.github.com`.');
	expect(out.length).toBe(5);
	expect(textOf(out[1] as never)).toBe('- "GET /user"');
});

test('only a repo rules address opens the rules page', () => {
	expect(rulesAt('#/repos/acme/web/rules')).toBe('acme/web');
	expect(rulesAt('#/repos/acme/web')).toBeNull();
	expect(rulesAt('#/repos/acme/web/rules/x')).toBeNull();
});

const said = (at: string, text: string) => ({ id: 1, at, kind: 'said', class: 'message' as const, text });

test("the agent's reply is its newest yaml block since the task was asked, and only words once it stopped", () => {
	const detail = (timeline: ReturnType<typeof said>[], state = 'working') => ({ task_at: '2026-11-26T10:00:00Z', state: state as 'working', timeline });
	const old = said('2026-11-26T09:00:00Z', 'Old.\n```yaml\nrules: []\n```');
	expect(replyOf(detail([old]), true)).toEqual({ at: 'waiting' });
	const edit = said('2026-11-26T10:00:05Z', 'Added GET /user under api.github.com.\n```yaml\nrules:\n  - GET /user\n```');
	expect(replyOf(detail([old, edit]), true)).toEqual({ at: 'proposed', note: 'Added GET /user under api.github.com.', text: 'rules:\n  - GET /user\n' });
	const words = said('2026-11-26T10:00:05Z', 'Which repo do you mean?');
	expect(replyOf(detail([words]), true)).toEqual({ at: 'waiting' });
	expect(replyOf(detail([words], 'done'), false)).toEqual({ at: 'answered', note: 'Which repo do you mean?' });
});

test('the diff numbers the lines the change leaves and tags what it added and took out', () => {
	const view = RulesDiff({
		lines: [
			{ text: 'allow:', kind: 'same' },
			{ text: '  - GET /user', kind: 'del' },
			{ text: '  - POST /x', kind: 'add' },
		],
	});
	expect(all(view, 'div').slice(1).map((d) => textOf(d))).toEqual(['1allow:', '  - GET /userremoved', '2  - POST /xnew']);
});
