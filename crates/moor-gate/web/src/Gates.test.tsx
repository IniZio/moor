import { expect, test } from 'bun:test';

import { ApproveView, type ApproveProps, GateList, StuckView } from './Gates';
import type { Wait } from './gate';
import { all, button, textOf } from './testkit';

const base: Wait = { gate: 'review', kind: 'approve', seq: 4, try_n: 1, why: null, handed: ['plan.md', 'after.png'], note: 'Ready for you.', command: null, tail: null, missing: [], hint: null };

const props = (over: Partial<ApproveProps> = {}): ApproveProps => ({
	box: 'web/gated',
	wait: base,
	busy: false,
	refusing: false,
	note: '',
	error: '',
	onYes: () => {},
	onNo: () => {},
	onSendNo: () => {},
	onBack: () => {},
	onNote: () => {},
	...over,
});

test('the approval card says what is asked, lists what the agent handed in and wrote, and offers Yes and No', () => {
	const card = ApproveView(props());
	const text = textOf(card);
	expect(text).toContain('The agent asks you to approve “review”.');
	expect(text).toContain('Ready for you.');
	expect(text).not.toContain('`');
	expect(button(card, 'Yes')).toBeDefined();
	expect(button(card, 'No')).toBeDefined();
	expect(all(card, 'input')).toHaveLength(0);
	const links = all(card, 'a').map((a) => [textOf(a), a.props.href]);
	expect(links).toEqual([
		['plan.md', '#/box/web%2Fgated/evidence'],
		['after.png', '#/box/web%2Fgated/evidence'],
	]);
});

test('pressing the buttons runs Yes, and No opens the note before anything is sent', () => {
	const sent: string[] = [];
	const card = ApproveView(props({ onYes: () => sent.push('yes'), onNo: () => sent.push('no-opened'), onSendNo: () => sent.push('no-sent') }));
	button(card, 'Yes')?.props.onClick();
	button(card, 'No')?.props.onClick();
	expect(sent).toEqual(['yes', 'no-opened']);
	const asking = ApproveView(props({ refusing: true, note: 'use blue', onSendNo: () => sent.push('no-sent') }));
	expect(button(asking, 'Yes')).toBeUndefined();
	expect(all(asking, 'input')[0]?.props.value).toBe('use blue');
	expect(button(asking, 'Send No')).toBeDefined();
	expect(button(asking, 'Back')).toBeDefined();
});

test('an approval with nothing handed in and no note leaves those lines out, and an error is shown', () => {
	const text = textOf(ApproveView(props({ wait: { ...base, handed: [], note: null }, error: 'nothing asks you' })));
	expect(text).not.toContain('handed in');
	expect(text).not.toContain('The agent wrote');
	expect(text).toContain('nothing asks you');
});

const stuck: Wait = { ...base, kind: 'stuck', try_n: null, handed: [], note: null, why: "out of tries: check 1 (`sh -c 'echo boom; exit 3'`) exited 3", command: "sh -c 'echo boom; exit 3'", tail: 'boom', hint: null };

test('the stuck card says which gate, why, the failing command and its output, and what the person can do', () => {
	const card = StuckView({ wait: stuck });
	const text = textOf(card);
	expect(text).toContain('“review” is stuck.');
	expect(text).toContain('Check 1 exited with code 3. No tries left.');
	expect(text).toContain("sh -c 'echo boom; exit 3'");
	expect(text).toContain('boom');
	expect(text).toContain('write to the agent below');
	expect(text).not.toContain('`');
	expect(all(card, 'details')).toHaveLength(0);
	expect(all(card, 'pre').length).toBe(2);
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

test('the gate list names each gate with where it stands, and is absent for a task without gates', () => {
	const list = GateList({ gate: { gates: ['plan', 'tests', 'review'], status: { state: 'open', gate: 'tests', try_n: 1, next: 'run_checks' } } });
	expect(all(list, 'li').map(textOf)).toEqual([
		'✓planpassed',
		'◐testsin progress · moor is running its checks',
		'○reviewnot started',
	]);
	expect(GateList({ gate: null })).toBeNull();
	expect(GateList({ gate: { gates: [], status: { state: 'passed' } } })).toBeNull();
	const done = GateList({ gate: { gates: ['plan', 'review'], status: { state: 'passed' } } });
	expect(all(done, 'li').every((li) => textOf(li).endsWith('passed'))).toBe(true);
});
