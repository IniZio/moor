import { expect, test } from 'bun:test';

import { ApproveView, type ApproveProps, retryGate, RulesView, StuckView } from './Gates';
import type { GateDoc, OutcomeDoc, Wait } from './gate';
import { all, button, LOST, NOT_RUN, textOf } from './testkit';

const base: Wait = { gate: 'review', kind: 'approve', seq: 4, try_n: 1, why: null, handed: ['plan.md', 'after.png'], note: 'Ready for you.', command: null, tail: null, missing: [], hint: null };

const props = (over: Partial<ApproveProps> = {}): ApproveProps => ({
	box: 'web/gated',
	wait: base,
	busy: false,
	note: '',
	error: '',
	images: [],
	pr: null,
	onYes: () => {},
	onNo: () => {},
	onNote: () => {},
	...over,
});

test('the approval card says what it waits for, shows what the agent handed in and wrote, and offers a note and yes or no', () => {
	const card = ApproveView(props({ pr: 12 }));
	const text = textOf(card);
	expect(textOf(all(card, 'h2')[0])).toBe('Waiting for your yes on “review”');
	expect(text).toContain('The agent handed in:');
	expect(textOf(all(card, 'blockquote')[0])).toBe('Ready for you. PR #12 is open.');
	expect(text).not.toContain('`');
	expect(button(card, 'Yes, it’s done')).toBeDefined();
	expect(button(card, 'No, send it back')).toBeDefined();
	expect(all(card, 'textarea')[0]?.props.placeholder).toBe('Add a note for the agent (optional)');
	const links = all(card, 'a').map((a) => [a.props.title, a.props.href]);
	expect(links).toEqual([
		['plan.md', '#/box/web%2Fgated/evidence'],
		['after.png', '#/box/web%2Fgated/evidence'],
	]);
});

test('the note is written beside the buttons, and each button answers at once', () => {
	const sent: string[] = [];
	const card = ApproveView(props({ note: 'use blue', onYes: () => sent.push('yes'), onNo: () => sent.push('no'), onNote: (t) => sent.push(`note:${t}`) }));
	expect(all(card, 'textarea')[0]?.props.value).toBe('use blue');
	all(card, 'textarea')[0]?.props.onInput?.({ currentTarget: { value: 'use red' } });
	button(card, 'Yes, it’s done')?.props.onClick();
	button(card, 'No, send it back')?.props.onClick();
	expect(sent).toEqual(['note:use red', 'yes', 'no']);
});

test('an approval with nothing handed in and no note leaves those lines out, and an error is shown', () => {
	const text = textOf(ApproveView(props({ wait: { ...base, handed: [], note: null }, error: 'nothing asks you' })));
	expect(text).not.toContain('handed in');
	expect(all(ApproveView(props({ wait: { ...base, handed: [], note: null } })), 'blockquote')).toHaveLength(0);
	expect(text).toContain('nothing asks you');
});

const stuck: Wait = { ...base, kind: 'stuck', try_n: null, handed: [], note: null, why: "out of tries: check 1 (`sh -c 'echo boom; exit 3'`) exited 3", command: "sh -c 'echo boom; exit 3'", tail: 'boom', hint: null };

test('the stuck card says which gate, why, the failing command and its output, and what the person can do', () => {
	const card = StuckView({ wait: stuck });
	const text = textOf(card);
	expect(textOf(all(card, 'h2')[0])).toBe('Stuck on “review”');
	expect(text).toContain('Check 1 exited with code 3. No tries left');
	expect(textOf(all(card, 'code')[0])).toBe("sh -c 'echo boom; exit 3'");
	expect(text).toContain('boom');
	expect(text).toContain('You can write to the agent below, open the terminal and fix it yourself, or give it one more try.');
	expect(text).not.toContain('`');
	expect(all(card, 'details')).toHaveLength(0);
	expect(all(card, 'pre').length).toBe(1);
});

test('a long output is cut to its last lines, with the whole of it behind a click', () => {
	const tail = Array.from({ length: 9 }, (_, i) => `line ${i + 1}`).join('\n');
	const card = StuckView({ wait: { ...stuck, tail } });
	const pres = all(card, 'pre').map(textOf);
	expect(pres).toContain('line 6\nline 7\nline 8\nline 9');
	expect(all(card, 'details')).toHaveLength(1);
	expect(pres.some((p) => p.startsWith('line 1\n'))).toBe(true);
});

test('a stuck card with missing evidence names what was not handed in, and has no command block', () => {
	const card = StuckView({ wait: { ...stuck, command: null, tail: null, why: 'out of tries: evidence refused twice, missing: proof.md', missing: ['proof.md'] } });
	const text = textOf(card);
	expect(text).toContain('The agent did not hand in');
	expect(all(card, 'li').map(textOf)).toEqual(['proof.md']);
	expect(text).not.toContain('The check that failed');
});

const SAYS = 'The pull request has exactly one evidence comment for this commit, and a moor/evidence status on it.';
const LONG = 'sha=$MOOR_PULL_HEAD_SHA; echo "<!-- moor-evidence sha=$sha -->" | grep -c . || { echo "no comment"; exit 1; }';
const said: Wait = { ...stuck, gate: 'done', says: SAYS, command: LONG, why: 'out of tries: check 1 (“' + SAYS + '”) exited 1', tail: 'want 1 moor-evidence comment, found 0' };

test('a stuck card with a sentence says what must be true and what moor ran after each try, then what it printed', () => {
	const card = StuckView({ wait: { ...said, try_n: 2 } });
	const text = textOf(card);
	const at = (s: string) => text.indexOf(s);
	expect(textOf(all(card, 'h2')[0])).toBe('Stuck on “done”');
	expect(text).toContain(`What must be true: ${SAYS} moor ran ${LONG} after each of the agent’s two tries.`);
	expect(at(SAYS)).toBeLessThan(at('want 1 moor-evidence comment'));
	expect(text).toContain('Check 1 exited with code 1. No tries left');
	expect(all(card, 'details')).toHaveLength(0);
});

const gates: GateDoc[] = [
	{ name: 'proof', evidence: ['pr-*'] },
	{ name: 'done', evidence: ['posted.md'], checks: [{ run: 'test -f never.txt', says: 'The file never.txt exists.' }] },
	{ name: 'ship', approval: 'owner', max_tries: 1 },
];

test('the workflow page says what moor holds a run to, one sentence per gate, in order, and ends with delivery', () => {
	const view = RulesView({ gates, asJson: false, onToggle: () => {} });
	const rows = all(view, 'li').map(textOf);
	expect(rows[0]).toBe('1. proof — the agent must hand in files named pr-*.');
	expect(rows[1]).toContain('2. done — the agent must hand in posted.md, and moor checks: The file never.txt exists. Two tries.');
	expect(rows[2]).toBe("3. ship — the workflow's owner must say yes. One try.");
	expect(textOf(view)).toContain('Then the run is delivered.');
	expect(all(view, 'details')).toHaveLength(1);
	expect(textOf(all(view, 'pre')[0])).toBe('test -f never.txt');
	expect(all(view, 'textarea')).toHaveLength(0);
});

test('the gates JSON sits behind the toggle, read-only', () => {
	const calls: string[] = [];
	const view = RulesView({ gates, asJson: true, onToggle: () => calls.push('toggled') });
	button(view, 'Edit as JSON')?.props.onClick();
	expect(calls).toEqual(['toggled']);
	const [box] = all(view, 'textarea');
	expect(box?.props.readOnly).toBe(true);
	expect(JSON.parse(String(box?.props.value))).toEqual(gates);
	expect(all(view, 'ol')).toHaveLength(0);
});

const none: Wait = {
	...stuck,
	gate: 'outcome',
	why: 'out of tries: no outcome held',
	command: null,
	tail: 'every outcome, as one blob',
	tried: [
		{ outcome: 'check passed', says: 'The newest status is success.', command: 'test "$s" = success', exit: 1, tail: 'the newest status is none' },
		{ outcome: 'check failed', says: null, command: 'false', exit: 2, tail: '' },
	],
};

test('the stuck card of the closing stage lists each outcome tried with its sentence or name, exit, output and command', () => {
	const card = StuckView({ wait: none });
	const text = textOf(card);
	expect(text).toContain('No outcome holds.moor tried each outcome twice.');
	expect(text).not.toContain('“outcome” is stuck');
	expect(text).not.toContain('yet');
	expect(text).not.toContain('No outcome held');
	expect(text).not.toContain('one blob');
	const items = all(card, 'li').map(textOf);
	expect(items[0]).toContain('The newest status is success.');
	expect(items[0]).toContain('Exited with code 1.');
	expect(items[0]).toContain('the newest status is none');
	expect(items[1]).toContain('check failed');
	expect(items[1]).toContain('Exited with code 2.');
	expect(all(card, 'details')).toHaveLength(2);
	expect(all(card, 'pre').map(textOf)).toEqual(expect.arrayContaining(['test "$s" = success', 'false']));
});

const outcomes: OutcomeDoc[] = [
	{ name: 'nothing to do', says: 'There is no open pull request.', tone: 'plain', anytime: true, checks: [{ run: '[ -z "$N" ]', says: 'No pull request is named.' }] },
	{ name: 'check failed', says: 'The check failed.', tone: 'bad', checks: ['false'] },
	{ name: 'done', says: 'The run delivered.' },
];

test('the workflow page lists how a run ends in place of the static delivery line', () => {
	const view = RulesView({ gates, outcomes, asJson: false, onToggle: () => {} });
	const text = textOf(view);
	expect(text).toContain('A run ends as one of');
	expect(text).not.toContain('Then the run is delivered.');
	const rows = all(view, 'li').map(textOf).slice(gates.length);
	expect(rows).toEqual([
		'nothing to do — There is no open pull request. · at any pointmoor checks: No pull request is named.Show the check[ -z "$N" ]',
		'check failed — The check failed.moor checks: a command exits with code 0Show the checkfalse',
		'done — The run delivered. · when no other holds',
	]);
});

test('the JSON behind the toggle holds the outcomes beside the gates', () => {
	const [box] = all(RulesView({ gates, outcomes, asJson: true, onToggle: () => {} }), 'textarea');
	expect(JSON.parse(String(box?.props.value))).toEqual({ gates, outcomes });
});

const WHY = 'moor, not the agent, got no answer from its checks twice in a row; the last was check 1 (`make`): Box 3 is busy with another command';
const HINT = 'moor, not the agent, got no answer from its checks. Send the agent a message to reopen the gate and run them again, or close the Box.';

test('a check moor got no answer from reads as moor\'s trouble: the heading, the check, why, then what to do', () => {
	const busy = StuckView({ wait: { ...stuck, gate: 'tests', why: WHY, says: null, tail: null, hint: HINT, unanswered: NOT_RUN } });
	expect(textOf(busy)).toBe(`moor could not run “tests”This is moor’s trouble, not the agent’s. moor tried twice. Box 3 is busy with another command.makeThe agent is waiting. Nothing it did is lost.${HINT}`);
	const lost = StuckView({ wait: { ...stuck, gate: 'outcome', why: WHY, tail: null, hint: HINT, unanswered: LOST } });
	expect(textOf(lost)).toBe(`moor got no answer from its checks.This is moor’s trouble, not the agent’s. moor tried twice. The check's output was lost.The pull request is merged.Show the checkgh pr viewThe agent is waiting. Nothing it did is lost.${HINT}`);
});

test('a card stuck for want of answers with no hint says that sending reruns the checks, not that it gives fresh tries', () => {
	const text = textOf(StuckView({ wait: { ...stuck, hint: null, unanswered: LOST } }));
	expect(text).toContain('Sending reopens the gate and moor runs its checks again.');
	expect(text).not.toContain('fresh tries');
	expect(text).not.toContain('No outcome holds');
});

test('a stuck gate offers one more try and, on the task page, the terminal; the peek leaves the terminal out', () => {
	const tried: string[] = [];
	const retry = { busy: false, error: '', onRetry: () => tried.push('retry') };
	const card = StuckView({ wait: stuck, retry, terminal: '#/box/web%2Fgated/terminal' });
	button(card, 'Give it one more try')?.props.onClick?.();
	expect(tried).toEqual(['retry']);
	expect(all(card, 'a').find((a) => textOf(a) === 'Open the terminal')?.props.href).toBe('#/box/web%2Fgated/terminal');
	const peek = StuckView({ wait: stuck, inPeek: true, retry });
	expect(button(peek, 'Give it one more try')).toBeDefined();
	expect(all(peek, 'a').some((a) => textOf(a) === 'Open the terminal')).toBe(false);
	expect(textOf(StuckView({ wait: stuck, retry: { ...retry, error: 'the Box is asleep' } }))).toContain('the Box is asleep');
});

test('one more try is a steer to the agent, which reopens the gate', async () => {
	const calls: { url: string; body: unknown }[] = [];
	const real = globalThis.fetch;
	globalThis.fetch = Object.assign(
		async (url: string, init?: RequestInit) => {
			calls.push({ url, body: JSON.parse(String(init?.body)) });
			return new Response('{}', { status: 200, headers: { 'Content-Type': 'application/json' } });
		},
		{ preconnect: () => {} },
	);
	try {
		await retryGate('csrf', 'web/stuck', 'tests');
	} finally {
		globalThis.fetch = real;
	}
	expect(calls).toEqual([{ url: '/account/api/box/steer', body: { box: 'web/stuck', text: 'Give “tests” one more try.', interrupt: false } }]);
});
