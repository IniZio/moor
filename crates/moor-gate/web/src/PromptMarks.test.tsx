import { expect, test } from 'bun:test';

import type { ReadingView } from './api';
import { RulesView } from './Gates';
import { PromptMarks } from './PromptMarks';
import { all, textOf } from './testkit';

const reading = (over: Partial<ReadingView> = {}): ReadingView => ({
	state: 'read',
	stale: [],
	source: 'Post one comment. Never push.\n\nSay what you did not run.',
	runs: [{ text: 'Post one comment', phrase: 'a' }, { text: '. ' }, { text: 'Never push', phrase: 'b' }, { text: '.\n\nSay what you did not run.' }],
	phrases: [
		{ id: 'a', mark: 'held', effects: ['e1'], intents: [] },
		{ id: 'b', mark: 'asked', effects: [], intents: [] },
	],
	effects: [{ id: 'e1', kind: 'allow', line: 'POST /x', key: 'allow POST /x', phrases: ['a'] }],
	gates: null,
	may: {},
	...over,
});

const spans = (el: ReturnType<typeof PromptMarks>) => all(el, 'span');

test('the block says the source and marks each phrase by its style', () => {
	const el = PromptMarks({ reading: reading(), lit: new Set(), onHover: () => {} });
	expect(textOf(el)).toBe(reading().source);
	expect(spans(el).map((s) => [textOf(s), (s.props as { class?: string }).class])).toEqual([
		['Post one comment', 'psp k-enf'],
		['Never push', 'psp k-ask'],
	]);
});

test('a phrase that is lit says so', () => {
	const el = PromptMarks({ reading: reading(), lit: new Set(['b']), onHover: () => {} });
	expect(spans(el).map((s) => (s.props as { class?: string }).class)).toEqual(['psp k-enf', 'psp k-ask nlit']);
});

test('a run of a phrase the reading does not list is plain text', () => {
	const el = PromptMarks({ reading: reading({ runs: [{ text: 'Post one comment', phrase: 'zz' }, { text: '. Never push.\n\nSay what you did not run.' }] }), lit: new Set(), onHover: () => {} });
	expect(spans(el)).toHaveLength(0);
	expect(textOf(el)).toBe(reading().source);
});

test('a gate row the pointer is on a phrase of is lit', () => {
	const gates = [{ name: 'screenshots', checks: ['true'] }, { name: 'comment', checks: ['true'] }];
	const rows = (lit: string[]) => all(RulesView({ gates, asJson: false, onToggle: () => {}, lit }), 'li').map((li) => (li.props as { class?: string }).class ?? '');
	expect(rows([])).toEqual(['', '']);
	expect(rows(['comment'])).toEqual(['', 'lit']);
});

const COMMENT = 'POST /repos/{repo}/issues/*/comments';

const intentReading = (): ReadingView =>
	reading({
		table: 1,
		phrases: [
			{ id: 'a', mark: 'held', effects: ['e1'], intents: [{ intent: 'comment', polarity: 'do', repo: null, as: 'person', found: 'lines', already: ['GET /repos/{repo}/issues/*/comments'], row: true, unholdable: null }] },
			{ id: 'b', mark: 'unmapped', effects: [], intents: [{ intent: 'push-branch', polarity: 'never', repo: null, as: 'person', found: 'no_row', already: [], row: false, unholdable: null }] },
		],
		effects: [{ id: 'e1', kind: 'allow', line: COMMENT, key: `allow:github-api:${COMMENT}`, phrases: ['a'], from: 'table' }],
	});

test('a request moor has no rule for draws dashed and apart', () => {
	const el = PromptMarks({ reading: intentReading(), lit: new Set(), onHover: () => {} });
	expect(spans(el).map((s) => (s.props as { class?: string }).class)).toEqual(['psp k-enf', 'psp k-ask k-unm']);
});

test('a lit phrase with an intent shows its raw lines, the row and table they came from, and the lines already allowed', () => {
	const el = PromptMarks({ reading: intentReading(), lit: new Set(['a']), onHover: () => {} });
	const note = all(el, 'span').find((s) => (s.props as { class?: string }).class === 'pnote');
	expect(note).toBeDefined();
	const said = textOf(note as Parameters<typeof textOf>[0]);
	expect(said).toContain(COMMENT);
	expect(said).toContain('from comment, table 1');
	expect(said).toContain('GET /repos/{repo}/issues/*/comments already allowed');
});

test('a lit phrase moor has no rule for says so and holds no line', () => {
	const el = PromptMarks({ reading: intentReading(), lit: new Set(['b']), onHover: () => {} });
	const note = all(el, 'span').find((s) => (s.props as { class?: string }).class === 'pnote');
	const said = textOf(note as Parameters<typeof textOf>[0]);
	expect(said).toContain('moor has no rule for this');
	expect(said).not.toContain(COMMENT);
});

test('a slice-1 phrase shows no note when lit', () => {
	const el = PromptMarks({ reading: reading(), lit: new Set(['a']), onHover: () => {} });
	expect(all(el, 'span').some((s) => (s.props as { class?: string }).class === 'pnote')).toBe(false);
});
