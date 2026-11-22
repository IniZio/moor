import { expect, test } from 'bun:test';

import * as v from 'valibot';

import { LOST, NOT_RUN } from './testkit';
import { closingRow, type GateView, GateViewSchema, gateLine, gateRows, outcomeLine, plain, ruleSentence, tailView, whyWords } from './gate';

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

const nothing = { name: 'nothing to do', tone: 'plain', says: 'The pull request is closed.' } as const;
const failedCheck = { name: 'check failed', tone: 'bad', says: 'The pull request check failed.' } as const;

test('gates an anytime outcome ended the run before read not reached, and a count that is missing keeps them all passed', () => {
	const early: GateView = { gates, gates_passed: 1, status: { state: 'passed', outcome: nothing } };
	expect(gateRows(early).map((r) => [r.state, r.words])).toEqual([
		['passed', 'passed'],
		['later', 'ended'],
		['later', 'not reached'],
	]);
	expect(gateRows({ gates, status: { state: 'passed', outcome: nothing } }).map((r) => r.state)).toEqual(['passed', 'passed', 'passed']);
	expect(v.parse(GateViewSchema, { gates, gates_passed: 0, status: { state: 'passed', outcome: nothing } }).gates_passed).toBe(0);
});

test('the closing row names what moor found with its sentence, says moor is checking while it decides, and is absent otherwise', () => {
	const bad = closingRow({ gates, status: { state: 'passed', outcome: failedCheck } });
	expect(bad).toEqual({ name: 'Outcome', state: 'bad', words: 'check failed · The pull request check failed.' });
	expect(closingRow({ gates, status: { state: 'passed', outcome: { name: 'done', tone: 'good' } } })).toBeNull();
	expect(closingRow({ gates, status: { state: 'passed', outcome: { name: 'check passed', tone: 'good' } } })).toEqual({ name: 'Outcome', state: 'passed', words: 'check passed' });
	expect(closingRow({ gates, status: { state: 'passed', outcome: nothing } })?.state).toBe('plain');
	expect(closingRow({ gates, status: { state: 'open', gate: 'outcome', try_n: 1, next: 'decide' } })).toEqual({
		name: 'Outcome',
		state: 'working',
		words: 'moor is checking which outcome holds',
	});
	expect(closingRow({ gates, status: { state: 'stuck', gate: 'outcome', why: 'out of tries: no outcome held' } })?.state).toBe('stuck');
	expect(closingRow({ gates, status: { state: 'open', gate: 'tests', try_n: 1, next: 'agent' } })).toBeNull();
	expect(closingRow({ gates, status: { state: 'passed' } })).toBeNull();
});

test('the rail line for a stuck closing stage speaks of the outcome, not a gate named outcome', () => {
	const line = gateLine({ gates, status: { state: 'stuck', gate: 'outcome', why: 'out of tries: no outcome held' } });
	expect(line).toBe('No outcome holds. moor tried each outcome twice.');
	expect(gateLine(view({ state: 'stuck', gate: 'tests', why: 'out of tries: check 1 exited 2' }))).toContain('“tests” is stuck');
});

test('the rail line for a stage moor got no answer from says so for either reason, with no word of an outcome holding', () => {
	const why = 'moor, not the agent, got no answer from its checks twice in a row; the last was check 1';
	for (const unanswered of [NOT_RUN, LOST]) {
		expect(gateLine({ gates, status: { state: 'stuck', gate: 'outcome', why, unanswered } })).toBe('moor got no answer from its checks.');
		expect(gateLine(view({ state: 'stuck', gate: 'tests', why, unanswered }))).toBe('moor got no answer from its checks.');
	}
});

test('an outcome is one line of its name and what it says', () => {
	expect(outcomeLine({ name: 'check failed', says: 'It failed.' })).toBe('check failed · It failed.');
	expect(outcomeLine({ name: 'nothing to do' })).toBe('nothing to do');
});

test('a gate in hand when the run ended waits for nobody: its row says how the run ended and no line asks the person', () => {
	const asked = { gates, status: { state: 'open', gate: 'review', try_n: 1, next: 'ask' }, run_ended: 'incomplete' } as const;
	expect(gateRows(asked).map((r) => [r.state, r.words])).toEqual([
		['passed', 'passed'],
		['passed', 'passed'],
		['later', 'run incomplete'],
	]);
	expect(gateLine(asked)).toBeNull();
	const stuck = { gates, status: { state: 'stuck', gate: 'outcome', why: 'no outcome held' }, run_ended: 'timed_out' } as const;
	expect(closingRow(stuck)).toBeNull();
	expect(gateLine(stuck)).toBeNull();
	const failed = { gates, status: { state: 'failed', gate: 'plan', why: 'the owner said no' }, run_ended: 'failed' } as const;
	expect(gateLine(failed)).toBe('Failed at “plan”: The owner said no.');
	expect(v.parse(GateViewSchema, asked).run_ended).toBe('incomplete');
});
