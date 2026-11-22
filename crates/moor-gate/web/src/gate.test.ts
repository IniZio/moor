import { expect, test } from 'bun:test';

import { type GateView, gateLine, gateRows, plain, tailView, whyWords } from './gate';

const gates = ['plan', 'tests', 'review'];
const view = (status: GateView['status']): GateView => ({ gates, status });

test('gates before the one in hand passed and the ones after it have not started', () => {
	const rows = gateRows(view({ state: 'open', gate: 'tests', try_n: 1, next: 'run_checks' }));
	expect(rows.map((r) => [r.name, r.state])).toEqual([
		['plan', 'passed'],
		['tests', 'working'],
		['review', 'later'],
	]);
	expect(rows[1]?.words).toBe('in progress · moor is running its checks');
	expect(rows[2]?.words).toBe('not started');
});

test('an open gate says what moor waits for and which try it is on', () => {
	const at = (next: 'agent' | 'run_checks' | 'ask', try_n: number) => gateRows(view({ state: 'open', gate: 'plan', try_n, next }))[0];
	expect(at('agent', 1)?.words).toBe("in progress · waiting for the agent's evidence");
	expect(at('ask', 1)).toMatchObject({ state: 'asking', words: 'waiting for you' });
	expect(at('run_checks', 2)?.words).toBe('in progress · moor is running its checks · try 2');
});

test('a stuck gate needs the person, a failed one failed, and a passed Box has every gate passed', () => {
	expect(gateRows(view({ state: 'stuck', gate: 'review', why: 'the agent went quiet' })).map((r) => r.state)).toEqual(['passed', 'passed', 'stuck']);
	expect(gateRows(view({ state: 'failed', gate: 'plan', why: 'the owner said no' })).map((r) => r.state)).toEqual(['failed', 'later', 'later']);
	expect(gateRows(view({ state: 'passed' })).map((r) => r.words)).toEqual(['passed', 'passed', 'passed']);
});

test('the words for a reason carry no code marks and leave the command to the card', () => {
	const why = "out of tries: check 1 (`sh -c 'echo boom; exit 3'`) exited 3";
	expect(whyWords(why)).toBe('Check 1 exited with code 3. No tries left.');
	expect(whyWords('the agent went quiet')).toBe('The agent went quiet.');
});

test('the one-line gate story names a gate that waits for a person or failed, and is silent when nothing is asked', () => {
	expect(gateLine(view({ state: 'open', gate: 'review', try_n: 1, next: 'ask' }))).toBe('Waiting for your yes or no on “review”');
	expect(gateLine(view({ state: 'stuck', gate: 'tests', why: 'the agent went quiet' }))).toBe('“tests” is stuck: The agent went quiet.');
	expect(gateLine(view({ state: 'failed', gate: 'review', why: 'the owner said no: too wide' }))).toBe('Failed at “review”: The owner said no: too wide.');
	expect(gateLine(view({ state: 'open', gate: 'plan', try_n: 1, next: 'agent' }))).toBeNull();
	expect(gateLine(view({ state: 'passed' }))).toBeNull();
	expect(gateLine(null)).toBeNull();
});

test('plain words have no code marks and quote the gates they name', () => {
	expect(plain('Gate `review` has passed; see `proof.md`.', ['review'])).toBe('Gate “review” has passed; see proof.md.');
	expect(plain('stray ` mark')).toBe('stray  mark');
	expect(plain('nothing to do')).toBe('nothing to do');
});

test('an output of a few lines shows whole and a longer one shows its last lines with the rest kept', () => {
	expect(tailView('boom\n')).toEqual({ shown: 'boom', all: null });
	const long = ['1', '2', '3', '4', '5', '6'].join('\n');
	expect(tailView(long)).toEqual({ shown: '3\n4\n5\n6', all: long });
});
