import { expect, test } from 'bun:test';

import { GateStory, PromptFold } from './GateStory';
import type { GateView } from './gate';
import { all, textOf } from './testkit';

const card = <div class="t-wait">The card</div>;
const rows = (view: ReturnType<typeof GateStory>) => all(view, 'li').map(textOf);
const openRows = (view: ReturnType<typeof GateStory>) => all(view, 'details').filter((d) => d.props.open).map((d) => textOf(all(d, 'summary')[0]));
const cardCount = (view: ReturnType<typeof GateStory>) => all(view, 'div').filter((d) => d.props.class === 't-wait').length;

const docs = {
	gates: [
		{ name: 'proof', evidence: ['before.png', 'after.png'] },
		{ name: 'tests', checks: [{ run: 'make test', says: 'All tests pass.' }] },
		{ name: 'review', approval: 'owner', max_tries: 1 },
	],
	outcomes: [{ name: 'fixed', says: 'The fix is in.' }, { name: 'nothing to fix', says: 'It already works.' }],
};

const waiting: GateView = { gates: ['proof', 'tests', 'review'], status: { state: 'open', gate: 'review', try_n: 1, next: 'ask' } };

test('a gate that waits for a yes is the open row, with its card in place and what must be true above it; the others stay shut', () => {
	const view = GateStory({ gate: waiting, docs, card });
	expect(rows(view)[0]).toBe('✓1proofpassedThe agent must hand in before.png and after.png.');
	expect(rows(view)[2]).toContain('●3reviewwaiting for your yes');
	expect(openRows(view)).toEqual(['●3reviewwaiting for your yes']);
	const open = all(view, 'details').find((d) => d.props.open);
	expect(textOf(open)).toContain("The workflow's owner must say yes. One try.");
	expect(textOf(open)).toContain('The card');
	expect(cardCount(view)).toBe(1);
});

test('a stuck gate holds the stuck card', () => {
	const view = GateStory({ gate: { gates: ['proof', 'tests'], status: { state: 'stuck', gate: 'tests', why: 'out of tries' } }, docs, card });
	expect(openRows(view)).toEqual(['✗2testsstuck']);
	expect(textOf(all(view, 'details').find((d) => d.props.open))).toContain('moor checks: All tests pass. Two tries.');
	expect(cardCount(view)).toBe(1);
});

test('when every gate passed and an outcome was reached, the last row says how it ended with its line, and nothing is open', () => {
	const view = GateStory({
		gate: { gates: ['proof', 'tests', 'review'], status: { state: 'passed', outcome: { name: 'fixed', tone: 'good', says: 'The fix is in.' } } },
		docs,
		card: null,
	});
	expect(rows(view)).toHaveLength(4);
	expect(rows(view)[3]).toBe('✓ended as: fixedThe fix is in.');
	expect(openRows(view)).toEqual(['✓ended as: fixed']);
	expect(rows(view).slice(0, 3).every((r) => r.endsWith('passed') || r.includes('passed'))).toBe(true);
});

test('a task a workflow made lists its possible endings by name before it ends', () => {
	const view = GateStory({ gate: { gates: ['proof', 'tests', 'review'], status: { state: 'open', gate: 'tests', try_n: 1, next: 'run_checks' } }, docs, card: null });
	expect(rows(view).at(-1)).toBe('○ends as: fixed · nothing to fix');
	expect(rows(view)[1]).toContain('◐2testsin hand, moor is checking');
	expect(openRows(view)).toHaveLength(1);
});

test('a one-off task has names and states only, and no endings to list', () => {
	const view = GateStory({ gate: { gates: ['tests', 'review'], status: { state: 'open', gate: 'tests', try_n: 2, next: 'agent' } }, docs: null, card: null });
	expect(rows(view)).toEqual(['◐1teststhe agent is working toward it · try 2', '○2reviewlater']);
	expect(all(view, 'details')).toHaveLength(0);
});

test('a card for the closing stage sits in the outcome row', () => {
	const view = GateStory({ gate: { gates: ['proof'], status: { state: 'stuck', gate: 'outcome', why: 'no outcome holds' } }, docs: null, card });
	expect(rows(view).at(-1)).toContain('✗outcome');
	expect(cardCount(view)).toBe(1);
	expect(openRows(view)).toHaveLength(1);
});

test('a card that matches no row still shows once, above the list', () => {
	const view = GateStory({ gate: { gates: ['proof'], status: { state: 'passed' } }, docs: null, card });
	expect(cardCount(view)).toBe(1);
});

test('the prompt is plain text behind a click, and absent when there is none', () => {
	const view = PromptFold({ prompt: 'Fix the **avatar** crop.\nKeep it small.' });
	expect(textOf(all(view, 'summary')[0])).toBe('Show the whole prompt');
	expect(textOf(all(view, 'pre')[0])).toBe('Fix the **avatar** crop.\nKeep it small.');
	expect(all(view, 'details')[0]?.props.open).toBeUndefined();
	expect(PromptFold({ prompt: '  ' })).toBeNull();
});

test('an outcome that can end the run at any point says so among the endings', () => {
	const anytime = { ...docs, outcomes: [{ name: 'fixed' }, { name: 'nothing to fix', anytime: true }] };
	const view = GateStory({ gate: waiting, docs: anytime, card: null });
	expect(rows(view).at(-1)).toBe('○ends as: fixed · nothing to fix (at any point)');
});

test('a one-off gate that waits for a person reads what it holds the agent to off its wait, and says how many tries it took', () => {
	const stuckWait = { gate: 'tests', kind: 'stuck' as const, seq: 1, try_n: 2, why: 'exited with code 1', handed: [], note: null, command: 'make test', says: 'all tests pass', tail: null, missing: [], hint: null };
	const stuck = GateStory({ gate: { gates: ['proof', 'tests', 'review'], status: { state: 'stuck', gate: 'tests', why: 'exited with code 1' } }, docs: null, card: null, wait: stuckWait });
	expect(rows(stuck)).toEqual(['✓1proofpassed', '✗2testsstuck, tried twicemoor checks: all tests pass', '○3reviewlater']);
	const yes = GateStory({ gate: waiting, docs: null, card: null, wait: { ...stuckWait, gate: 'review', kind: 'approve', try_n: 1, says: null } });
	expect(rows(yes)[2]).toBe('●3reviewwaiting for your yesneeds your yes before it counts as done');
});
