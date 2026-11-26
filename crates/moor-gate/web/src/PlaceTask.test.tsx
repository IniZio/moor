import { beforeEach, expect, test } from 'bun:test';
import { type ComponentChild, isValidElement } from 'preact';

import { type BoxDetail, boxGet, type Entry } from './api';
import { AskedTask, AskView } from './AskedTask';
import { HeaderActions } from './Ending';
import { PlaceStory } from './PlaceStory';
import { Composer, SayView } from './Steer';
import { TaskHead } from './TaskHead';
import { all, textOf, walk, type El } from './testkit';

const at = (m: number) => new Date(2026, 10, 24, 17, 40 + m, 0).toISOString();
const you = (id: number, m: number, text: string): Entry => ({ id, at: at(m), kind: 'steered', class: 'you', text });
const said = (id: number, m: number, text: string): Entry => ({ id, at: at(m), kind: 'agent_said', class: 'message', text });
const err = (id: number, m: number, text: string): Entry => ({ id, at: at(m), kind: 'error', class: 'error', text });
const note = (id: number, text: string): Entry => ({ id, at: at(0), kind: 'cloned', class: 'routine', text });
const call = (id: number, m: number, title: string, tool: Partial<NonNullable<Entry['tool']>> = {}): Entry => ({
	id,
	key: `k${id}`,
	at: at(m),
	kind: 'tool',
	class: 'step',
	text: title,
	tool: { name: 'Bash', title, failed: false, refused: false, input: null, output: null, ...tool },
});
const failed = (id: number, m: number, title: string, output?: string) => call(id, m, title, { failed: true, output: output ? { text: output, cut: false } : null });

let reply = {};
beforeEach(() => {
	globalThis.fetch = Object.assign(async () => new Response(JSON.stringify(reply), { status: 200, headers: { 'Content-Type': 'application/json' } }), { preconnect: () => {} });
});

/** The task as the gate sends it for a question: no work Box behind it. */
async function detailOf(timeline: Entry[], state = 'done'): Promise<BoxDetail> {
	reply = { box: 'place-1', repo: 'acme/web', other_repos: [], title: 'where is the main entry point?', prompt: '', state, stale: false, line: '', updated_at: at(0), task_at: at(3), pr: null, timeline, artifacts: [] };
	return boxGet('place-1');
}
const story = async (timeline: Entry[], state = 'done') => PlaceStory({ detail: await detailOf(timeline, state), you: 'ann' });

const classes = (el: El) => String(el.props.class ?? '').split(' ');
const byClass = (view: ComponentChild, name: string) => {
	const found: El[] = [];
	walk(view, (el) => {
		if (classes(el).includes(name)) found.push(el);
	});
	return found;
};
/** The text of the page outside its folds. */
const outside = (view: ComponentChild) => textOf((Array.isArray(view) ? view : [view]).flat().filter((c) => !(c as El).props?.class?.includes?.('t-pl-fold')));

const ASKED = [you(1, 3, 'where is the main entry point?')];

test('the person\'s message is a bubble with their name and the time above it', async () => {
	const view = await story([...ASKED, said(2, 4, 'It is src/main.rs.')]);
	const [row] = byClass(view, 't-pl-you');
	expect(row).toBeDefined();
	const [meta, bubble] = (row as El).props.children as El[];
	expect(textOf(meta)).toBe('ann · 17:43');
	expect(textOf(bubble)).toBe('where is the main entry point?');
	expect(classes(meta as El)).toContain('t-pl-meta');
	expect(classes(bubble as El)).toContain('t-pl-bubble');
});

test('the agent\'s answer is a diamond line with no speaker or time column', async () => {
	const view = await story([...ASKED, said(2, 4, 'It is **src/main.rs**.')]);
	const [line] = byClass(view, 't-pl-say');
	expect(textOf(line)).toMatch(/^◆/);
	expect(textOf(line)).toContain('src/main.rs');
	expect(textOf(view)).not.toMatch(/Agent|17:44/);
});

test('every tool call of a turn folds into one quiet line under the answer, and a failure the agent got past stays inside it', async () => {
	const view = await story([
		...ASKED,
		note(5, 'cloned IniZio/moor-pilot-example'),
		failed(6, 3, 'Ran git clone (exit 128)'),
		err(7, 4, 'credential withheld: no allow entry matches the request'),
		call(8, 4, 'Ran gh api repos/x/git/trees'),
		call(9, 4, 'Read src/main.rs', { name: 'Read', group: 'read' }),
		call(10, 4, 'Ran gh api repos/x/contents'),
		said(11, 5, 'It is src/main.rs.'),
	]);
	const folds = byClass(view, 't-pl-fold');
	expect(folds).toHaveLength(1);
	const [fold] = folds as [El];
	const [summary] = all(fold, 'summary');
	expect(textOf(summary)).toContain('Read 1 file, ran 3 commands');
	const parts = (view as El).props.children as El[];
	const kinds = parts.flat().map((p) => classes(p as El).find((c) => c.startsWith('t-pl-')));
	expect(kinds).toEqual(['t-pl-you', 't-pl-say', 't-pl-fold']);
	expect(textOf(fold)).toContain('credential withheld');
	expect(textOf(fold)).toContain('exit 128');
	expect(outside(parts)).not.toMatch(/credential|exit 128|Error|Something went wrong/);
	expect(byClass(view, 't-pl-ended')).toHaveLength(0);
});

test('the lines that only set up a work Box are not shown', async () => {
	const view = await story([...ASKED, note(5, 'cloned IniZio/moor-pilot-example'), said(2, 4, 'Done.')]);
	expect(textOf(view)).not.toMatch(/cloned|setup note/);
});

test('a turn with calls and no answer yet shows the fold alone', async () => {
	const view = await story([...ASKED, call(6, 3, 'Ran ls'), call(7, 3, 'Ran cat x')], 'working');
	expect(byClass(view, 't-pl-say')).toHaveLength(0);
	expect(textOf(byClass(view, 't-pl-fold')[0])).toContain('Ran 2 commands');
});

test('a failure that ends the task is the one thing that shows as an error, outside the fold', async () => {
	const view = await story([...ASKED, call(6, 3, 'Ran ls'), failed(7, 4, 'Ran git clone (exit 128)', 'fatal: could not read Username')], 'failed');
	const [ended] = byClass(view, 't-pl-ended');
	expect(ended).toBeDefined();
	expect(textOf(ended)).toContain('exit 128');
	expect(textOf(ended)).toContain('could not read Username');
	expect(ended?.props.role).toBe('alert');
	expect(textOf(byClass(view, 't-pl-fold')[0])).toContain('Ran 1 command');
	expect(textOf(byClass(view, 't-pl-fold')[0])).not.toContain('exit 128');
});

test('an error line ends the task the same way when nothing follows it', async () => {
	const view = await story([...ASKED, err(7, 4, 'credential withheld: no allow entry matches the request')], 'failed');
	expect(textOf(byClass(view, 't-pl-ended')[0])).toContain('credential withheld');
});

test('the same failure is folded when the agent answered after it, or when the task did not fail', async () => {
	const recovered = await story([...ASKED, failed(6, 3, 'Ran git clone (exit 128)'), said(7, 4, 'Used the API instead.')], 'failed');
	expect(byClass(recovered, 't-pl-ended')).toHaveLength(0);
	const running = await story([...ASKED, failed(6, 3, 'Ran git clone (exit 128)')], 'working');
	expect(byClass(running, 't-pl-ended')).toHaveLength(0);
	expect(textOf(byClass(running, 't-pl-fold')[0])).toContain('exit 128');
});

test('a second question starts a second turn with its own bubble, answer and fold', async () => {
	const view = await story([...ASKED, call(6, 3, 'Ran ls'), said(7, 4, 'A.'), you(8, 6, 'and the tests?'), call(9, 7, 'Ran ls t'), said(10, 8, 'B.')]);
	expect(byClass(view, 't-pl-you')).toHaveLength(2);
	expect(byClass(view, 't-pl-say')).toHaveLength(2);
	expect(byClass(view, 't-pl-fold')).toHaveLength(2);
});

test('every message of the agent is its own diamond line, with the calls that led up to it folded under it', async () => {
	const view = await story([
		...ASKED,
		said(2, 3, 'Need gh; install it.'),
		call(3, 3, 'Ran apt-get install gh'),
		said(4, 4, 'Let me confirm.'),
		call(5, 4, 'Ran gh --version'),
		said(6, 5, 'It is **src/main.rs**.'),
	]);
	expect(byClass(view, 't-pl-say').map(textOf)).toEqual(['◆Need gh; install it.', '◆Let me confirm.', '◆It is src/main.rs.']);
	const kinds = ((view as El).props.children as El[]).flat().map((p) => classes(p).find((c) => c.startsWith('t-pl-')));
	expect(kinds).toEqual(['t-pl-you', 't-pl-say', 't-pl-say', 't-pl-fold', 't-pl-say', 't-pl-fold']);
	const [first, second] = byClass(view, 't-pl-fold').map(textOf);
	expect(first).toContain('apt-get');
	expect(first).not.toContain('gh --version');
	expect(second).toContain('gh --version');
});

test('a turn of words alone is one diamond line and no fold', async () => {
	const view = await story([...ASKED, said(2, 4, 'It is src/main.rs.')]);
	expect(byClass(view, 't-pl-say')).toHaveLength(1);
	expect(byClass(view, 't-pl-fold')).toHaveLength(0);
});

test('two messages and no calls are two diamond lines and no fold', async () => {
	const view = await story([...ASKED, said(2, 3, 'Looking.'), said(3, 4, 'Found it.')]);
	expect(byClass(view, 't-pl-say').map(textOf)).toEqual(['◆Looking.', '◆Found it.']);
	expect(byClass(view, 't-pl-fold')).toHaveLength(0);
});

test('a failure after the agent\'s last words still ends the task', async () => {
	const view = await story([...ASKED, said(2, 3, 'Trying.'), said(3, 4, 'Cloning.'), failed(6, 5, 'Ran git clone (exit 128)')], 'failed');
	expect(byClass(view, 't-pl-ended')).toHaveLength(1);
	expect(byClass(view, 't-pl-say').map(textOf)).toEqual(['◆Trying.', '◆Cloning.']);
});

/** What the task page puts at the right of its header. */
async function headerActions(state: string) {
	const detail = await detailOf([...ASKED, said(2, 4, 'x')], state);
	const view = AskedTask({ detail, error: '', now: 0, csrf: 'c', reload: () => {}, you: 'ann' });
	const head = [view.props.children].flat(3).find((c) => isValidElement(c) && c.type === TaskHead);
	return isValidElement(head) ? (head.props as { actions: ComponentChild }).actions : undefined;
}
const isStop = (a: ComponentChild) => isValidElement(a) && a.type === HeaderActions;

test('Stop shows while the agent works or waits on the person, and not once the task is done', async () => {
	expect(isStop(await headerActions('working'))).toBe(true);
	expect(isStop(await headerActions('needs_you'))).toBe(true);
	for (const state of ['done', 'failed', 'asleep', 'closed']) expect(isStop(await headerActions(state))).toBe(false);
});

const say = (over: Partial<Parameters<typeof SayView>[0]['s']> = {}) => ({
	plain: false,
	text: '',
	busy: false,
	error: '',
	waiting: [],
	refusal: undefined,
	how: 'steer' as const,
	asleep: false,
	placeholder: 'Write to the agent…',
	hint: 'It reaches the agent after its current step.',
	area: { current: null },
	...over,
});
const acts = { text: () => {}, mode: () => {}, send: () => {} };

test('the message box of a Box shows its hint and the Steer and Interrupt choice', async () => {
	const view = SayView({ s: say(), act: acts });
	expect(textOf(view)).toContain('Steer');
	expect(textOf(view)).toContain('Interrupt');
	expect(textOf(view)).toContain('It reaches the agent');
});

test('the message box of a question\'s task is one field and Send', async () => {
	const view = SayView({ s: say({ plain: true }), act: acts });
	expect(all(view, 'textarea')).toHaveLength(1);
	expect(all(view, 'button').map((b) => textOf(b))).toEqual(['Send']);
	expect(all(view, 'input')).toHaveLength(0);
	expect(textOf(view)).not.toMatch(/Steer|Interrupt|reaches the agent/);
});

test('a question\'s task page marks itself for its own density and asks for the plain message box', async () => {
	const detail = await detailOf([...ASKED, said(2, 4, 'x')]);
	const view = AskedTask({ detail, error: '', now: 0, csrf: 'c', reload: () => {}, you: 'ann' });
	expect(view.props.class).toContain('t-place');
	const composer = [view.props.children].flat(3).find((c) => isValidElement(c) && c.type === Composer);
	expect(isValidElement(composer) && composer.props.plain).toBe(true);
});

test('a message on a later day than the task began says the day as well as the time', async () => {
	const next = new Date(2026, 10, 25, 9, 12).toISOString();
	const view = await story([...ASKED, said(2, 4, 'A.'), { id: 3, at: next, kind: 'steered', class: 'you', text: 'Round it.' }]);
	expect(byClass(view, 't-pl-meta').map(textOf)).toEqual(['ann · 17:43', 'ann · 25 Nov 09:12']);
});

test('the agent\'s question closes the conversation, after everything it said', async () => {
	const detail = await detailOf([...ASKED, said(2, 4, 'Wrote the spec.')]);
	const view = PlaceStory({ detail, you: 'ann', end: <section class="t-pl-ask" /> });
	const kinds = ((view as El).props.children as El[]).flat().map((p) => classes(p).find((c) => c.startsWith('t-pl-')));
	expect(kinds).toEqual(['t-pl-you', 't-pl-say', 't-pl-ask']);
});

test('the question shows its answers as small buttons, the first one filled, and a link to answer in your own words', () => {
	const picked: number[] = [];
	let own = 0;
	const needs = { id: 'q1', kind: 'question' as const, question: 'Build it like this?', detail: null, picks: [{ n: 1, label: 'Build it', role: 'choice' as const }, { n: 2, label: 'Change the spec', role: 'choice' as const }], other: 3, approve: null, plan: false, gate: null };
	const view = AskView({ needs, busy: false, error: '', pick: (n) => picked.push(n), own: () => own++ });
	const buttons = all(view, 'button');
	expect(buttons.map(textOf)).toEqual(['Build it', 'Change the spec', 'or answer in your own words']);
	expect(buttons.map((b) => classes(b).join(' '))).toEqual(['t-qbtn t-qbtn-primary', 't-qbtn', 't-qlnk']);
	expect(textOf(view)).not.toContain('Your input is needed');
	buttons[1]?.props.onClick?.();
	buttons[2]?.props.onClick?.();
	expect(picked).toEqual([2]);
	expect(own).toBe(1);
});

test('the head of a question\'s task leaves the pull request to its card', async () => {
	const detail = { ...(await detailOf([...ASKED])), pr: { repo: 'acme/web', number: 14, url: 'https://github.com/acme/web/pull/14' } };
	expect(textOf(TaskHead({ t: detail, now: 0, actions: null }))).not.toContain('PR #14');
});
