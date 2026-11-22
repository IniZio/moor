import { expect, test } from 'bun:test';

import * as v from 'valibot';

import { type GateView, GateViewSchema, gateLine, gateRows, plain, ruleSentence, tailView, whyWords } from './gate';

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

test('while moor decides the outcome every gate reads passed, and a view with an outcome parses', () => {
	const deciding = view({ state: 'open', gate: 'outcome', try_n: 1, next: 'decide' });
	expect(gateRows(deciding).map((r) => r.state)).toEqual(['passed', 'passed', 'passed']);
	expect(gateRows(view({ state: 'stuck', gate: 'outcome', why: 'out of tries: no outcome held' })).map((r) => r.state)).toEqual([
		'passed',
		'passed',
		'passed',
	]);
	const passed = { gates, status: { state: 'passed', outcome: { name: 'check failed', tone: 'bad' } } };
	expect(v.parse(GateViewSchema, passed).status.state).toBe('passed');
	expect(v.parse(GateViewSchema, { gates, status: deciding.status }).status).toMatchObject({ next: 'decide' });
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

test('a reason that names a check by its sentence keeps it in a list line and drops it where the card already says it', () => {
	const why = 'out of tries: check 1 (“The file never.txt exists.”) exited 1';
	expect(whyWords(why)).toBe('Check 1 (“The file never.txt exists.”) exited with code 1. No tries left.');
	expect(whyWords(why, 'The file never.txt exists.')).toBe('Check 1 exited with code 1. No tries left.');
});

test('a gate reads as a sentence: what the agent hands in, what moor checks, who says yes, how many tries', () => {
	expect(ruleSentence({ name: 'proof', evidence: ['pr-*'] })).toBe('the agent must hand in files named pr-*.');
	const says = 'The pull request has exactly one evidence comment for this commit, and a moor/evidence status on it.';
	expect(ruleSentence({ name: 'done', evidence: ['posted.md'], checks: [{ run: 'true', says }] })).toBe(
		`the agent must hand in posted.md, and moor checks: ${says} Two tries.`,
	);
	expect(ruleSentence({ name: 'done', evidence: ['a.md', 'b.md', 'c-*'], checks: ['true', { run: 'x', says: 'It holds.' }], max_tries: 1 })).toBe(
		'the agent must hand in a.md, b.md and files named c-*, and moor checks: a command exits with code 0; It holds. One try.',
	);
	expect(ruleSentence({ name: 'ship', approval: 'owner', max_tries: 3 })).toBe("the workflow's owner must say yes. Three tries.");
	expect(ruleSentence({ name: 'ship', evidence: ['x.md'], approval: 'owner' })).toBe(
		"the agent must hand in x.md. Then the workflow's owner must say yes. Two tries.",
	);
});
