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
		{ id: 'a', mark: 'held', effects: ['e1'] },
		{ id: 'b', mark: 'asked', effects: [] },
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
