import { beforeEach, expect, test } from 'bun:test';
import { type ComponentChild, isValidElement } from 'preact';

import { type BoxDetail, boxGet, type Entry } from './api';
import { AskedTask } from './AskedTask';
import { HeaderActions } from './Ending';
import { PlaceStory } from './PlaceStory';
import { FailedCard } from './Retry';
import { Composer } from './Steer';
import { TaskHead } from './TaskHead';
import { TerminalTab } from './Terminal';
import { all, drawn, textOf, walk, type El } from './testkit';

const at = (m: number) => new Date(2026, 10, 24, 9, m, 0).toISOString();
const PROMPT = 'Check every package for upgrades.';
const start = (): Entry => ({ id: 1, at: at(0), kind: 'steered', class: 'you', text: PROMPT });
const you = (id: number, m: number, text: string): Entry => ({ id, at: at(m), kind: 'steered', class: 'you', text });
const said = (id: number, m: number, text: string): Entry => ({ id, at: at(m), kind: 'agent_said', class: 'message', text });
const file = (name: string, version: number, m: number) => ({ name, version, kind: 'report', content_type: 'image/png', size: 2048, created_at: at(m), image: true, href: `/e/${name}/${version}` });
const PULL = 'https://github.com/acme/api/pull/212';

let reply = {};
beforeEach(() => {
	globalThis.fetch = Object.assign(async () => new Response(JSON.stringify(reply), { status: 200, headers: { 'Content-Type': 'application/json' } }), { preconnect: () => {} });
});

/** A workflow run as the gate sends it: its own Box, no question's time. */
/** What a test sets over the run the gate sends. */
type Over = Partial<Pick<BoxDetail, 'state' | 'failed' | 'retryable' | 'pr' | 'workflow' | 'artifacts' | 'outputs'>>;
async function runOf(timeline: Entry[], extra: Over = {}): Promise<BoxDetail> {
	reply = { box: 'deps/w1', repo: 'acme/api', other_repos: [], title: 'Daily dependency check, 24 Nov', prompt: PROMPT, state: 'done', stale: false, line: '', updated_at: at(20), pr: null, workflow: { name: 'deps' }, from_workflow: true, timeline, artifacts: [], outputs: [], ...extra };
	return boxGet('deps/w1');
}

const classes = (el: El) => String(el.props.class ?? '').split(' ');
const order = (view: ComponentChild) => ((view as El).props.children as El[]).flat().map((p) => classes(drawn(p)).find((c) => c.startsWith('t-pl-')));
const page = (detail: BoxDetail, tab: 'timeline' | 'terminal' = 'timeline') => AskedTask({ detail, error: '', now: 0, csrf: 'c', reload: () => {}, you: 'ana', tab });
const part = (view: El, type: El['type']) => [view.props.children].flat(3).find((c) => isValidElement(c) && c.type === type) as El | undefined;
const headOf = (view: El) => part(view, TaskHead) as El;

test('a run opens with the automation\'s line, its prompt folded behind show, and no bubble for it', async () => {
	const view = PlaceStory({ detail: await runOf([start(), said(2, 20, 'Five packages have upgrades.'), you(3, 41, 'Skip it this time')]), you: 'ana' });
	expect(order(view)).toEqual(['t-pl-first', 't-pl-say', 't-pl-you']);
	const [first] = (view as El).props.children as El[][];
	expect(textOf(first)).toBe(`Started by the automation, with its prompt. show${PROMPT}`);
	expect(textOf(view)).not.toContain('ana · 09:00');
});

test('what a run made sits after the words it ended with: its files as cards with versions, its outputs as cards in GitHub', async () => {
	const detail = await runOf([start(), said(2, 20, 'Five packages have upgrades.'), you(3, 41, 'Skip it'), said(4, 42, 'Left lodash at 4.')], {
		artifacts: [file('Upgrade report', 1, 20), file('Upgrade report', 2, 42)],
		outputs: [{ kind: 'pull request', url: PULL, title: 'Pull request #212', first_at: Date.parse(at(20)), last_at: Date.parse(at(20)) }],
	});
	const view = PlaceStory({ detail, you: 'ana' });
	expect(order(view)).toEqual(['t-pl-first', 't-pl-say', 't-pl-art', 't-pl-out', 't-pl-you', 't-pl-say', 't-pl-art']);
	const cards: El[] = [];
	walk(view, (el) => classes(el).includes('t-pl-art') && cards.push(el));
	expect(textOf(cards[0])).toContain('newer: v2');
	expect(classes(cards[0] as El)).toContain('t-pl-sup');
	expect(textOf(cards[1])).toContain('v1v2');
	expect(textOf(view)).toContain('Pull request #212in GitHub');
});

test('the pull request a run was started for is named on its head, not drawn as a card', async () => {
	const pr = { repo: 'acme/web', number: 482, url: 'https://github.com/acme/web/pull/482' };
	const detail = await runOf([said(2, 2, 'Ran the check.')], { workflow: { name: 'evidence', pull: 'acme/web#482' }, pr });
	expect(textOf(PlaceStory({ detail, you: 'ana' }))).not.toContain('Pull request #482');
	const head = TaskHead({ t: detail, now: 0, actions: null, place: true, asked: at(0) });
	expect(textOf(head)).toContain('started by evidence · pull request #482');
	expect(all(head, 'a').find((a) => textOf(a) === '#482')?.props.href).toBe(pr.url);
	expect(textOf(head)).not.toContain('moor/');
});

test('a scheduled run\'s head says which automation started it and the day and time it began', async () => {
	const head = TaskHead({ t: await runOf([start()]), now: Date.parse(at(30)), actions: null, place: true, asked: at(0) });
	expect(textOf(head)).toContain('started by deps · 24 Nov 09:00');
	expect(all(head, 'a').find((a) => textOf(a) === 'deps')?.props.href).toBe('#/workflows/deps');
});

test('a run keeps its terminal, changes and Close behind the head\'s menu, and Stop only while the agent runs', async () => {
	const done = headOf(page(await runOf([start()]))).props.actions as El;
	expect(done.type).toBe(HeaderActions);
	expect(done.props).toMatchObject({ part: 'more', changes: true });
	const working = headOf(page(await runOf([start()], { state: 'working' }))).props.actions as El;
	expect((working.props as { part?: string }).part).toBeUndefined();
});

test('a run steers and interrupts from the message box while the agent works, and is one field otherwise', async () => {
	const plain = async (state: string) => (part(page(await runOf([start()], { state })), Composer)?.props as { plain: boolean }).plain;
	expect(await plain('working')).toBe(false);
	expect(await plain('needs_you')).toBe(true);
	expect(await plain('done')).toBe(true);
});

test('a run whose agent failed shows no Start again; a run whose setup failed keeps it', async () => {
	expect(part(page(await runOf([start()], { state: 'failed', failed: true })), FailedCard)).toBeUndefined();
	expect(part(page(await runOf([start()], { state: 'failed', failed: true, retryable: true })), FailedCard)).toBeDefined();
});

test('the terminal of a run opens in place of its conversation, with a way back', async () => {
	const view = page(await runOf([start()], { state: 'working' }), 'terminal');
	const panel = [view.props.children].flat(3).find((c) => isValidElement(c) && classes(c as El).includes('t-pl-view')) as El;
	expect([panel.props.children].flat().some((c) => isValidElement(c) && c.type === TerminalTab)).toBe(true);
	expect([view.props.children].flat(3).some((c) => isValidElement(c) && classes(c as El).includes('t-pl-conv'))).toBe(false);
	const back = [panel.props.children].flat().find((c) => isValidElement(c) && c.type === 'a') as El;
	expect(textOf(back)).toContain('Back to the conversation');
	expect(back.props.href).toBe('#/box/deps%2Fw1');
});
