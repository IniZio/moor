import { beforeEach, expect, test } from 'bun:test';
import { type ComponentChild } from 'preact';

import { type BoxDetail, boxGet, type Entry } from './api';
import { PlaceStory } from './PlaceStory';
import { all, drawn, textOf, walk, type El } from './testkit';

const at = (m: number, s = 0) => new Date(2026, 10, 24, 11, m, s).toISOString();
const you = (id: number, m: number, text: string): Entry => ({ id, at: at(m), kind: 'steered', class: 'you', text });
const said = (id: number, m: number, text: string, s = 0): Entry => ({ id, at: at(m, s), kind: 'agent_said', class: 'message', text });

const WORK = 'clinic/mcp-1a2b3c4d';
const ARTIFACT = (name: string, image: boolean, m: number) => ({
	name,
	version: 1,
	kind: image ? 'screenshot' : 'report',
	content_type: image ? 'image/png' : 'text/plain',
	size: 20480,
	created_at: at(m),
	image,
	href: `/account/api/box/artifact?box=clinic%2Fmcp-1a2b3c4d&name=${encodeURIComponent(name)}&version=1`,
});
/** What a test sets over the work Box the gate sends. */
type Over = { state?: string; ready_ms?: number | null; closed_at?: string | null; started_at?: string; artifacts?: object[] };
const WORK_BOX = (extra: Over = {}) => ({
	id: 1,
	name: WORK,
	repo: 'acme/clinic-prototype',
	started_at: at(5, 10),
	ready_ms: 38_000,
	state: 'running',
	closed_at: null,
	artifacts: [],
	...extra,
});

let reply = {};
beforeEach(() => {
	globalThis.fetch = Object.assign(async () => new Response(JSON.stringify(reply), { status: 200, headers: { 'Content-Type': 'application/json' } }), { preconnect: () => {} });
});

/** The task as the gate sends it for a question whose place started work Boxes; `extra` is set over it. */
/** What a test sets over the task the gate sends. */
type Task = { task_boxes?: object[]; pr?: { repo: string; number: number; url: string } };
async function detailOf(timeline: Entry[], extra: Task = {}): Promise<BoxDetail> {
	reply = { box: 'place-1', repo: 'acme/clinic-prototype', other_repos: [], title: 'Where is the booking fee calculated?', prompt: '', state: 'done', stale: false, line: '', updated_at: at(0), task_at: at(0), pr: null, timeline, artifacts: [], ...extra };
	return boxGet('place-1');
}
const story = async (timeline: Entry[], extra: Task = {}) => PlaceStory({ detail: await detailOf(timeline, extra), you: 'Maya' });

const classes = (el: El) => String(el.props.class ?? '').split(' ');
const byClass = (view: ComponentChild, name: string) => {
	const found: El[] = [];
	walk(view, (el) => {
		if (classes(el).includes(name)) found.push(el);
	});
	return found;
};
/** The classes of the story's parts, in order. */
const order = (view: ComponentChild) => ((view as El).props.children as El[]).flat().map((p) => classes(drawn(p)).find((c) => c.startsWith('t-pl-')));

const TWO_TURNS = [you(1, 0, 'Where is the booking fee calculated?'), said(2, 1, 'In src/booking/fee.ts.'), you(3, 4, 'Make it 3% above 500.'), said(4, 5, 'That means running the app, so I started a Box.'), said(5, 10, 'Changed the rate.')];

test('a work Box is one line in the turn that holds its start, above the answer of the agent', async () => {
	const view = await story(TWO_TURNS, { task_boxes: [WORK_BOX()] });
	const lines = byClass(view, 't-pl-box');
	expect(lines).toHaveLength(1);
	expect(order(view)).toEqual(['t-pl-you', 't-pl-say', 't-pl-you', 't-pl-box', 't-pl-say', 't-pl-fold']);
});

test('a running Box line names the repo and the ready time and opens the Box terminal', async () => {
	const view = await story(TWO_TURNS, { task_boxes: [WORK_BOX()] });
	const [line] = byClass(view, 't-pl-box') as [El];
	expect(textOf(line)).toBe('▸ Box · acme/clinic-prototype · ready in 38s · running');
	expect(line.type).toBe('a');
	expect(line.props.href).toBe('#/box/clinic%2Fmcp-1a2b3c4d/terminal');
});

test('a closed Box line says how long it ran and is no link', async () => {
	const closed = WORK_BOX({ state: 'closed', closed_at: at(11, 12) });
	const view = await story(TWO_TURNS, { task_boxes: [closed] });
	const [line] = byClass(view, 't-pl-box') as [El];
	expect(textOf(line)).toBe('▸ Box · acme/clinic-prototype · ready in 38s · closed after 6 min');
	expect(line.type).not.toBe('a');
	expect(all(line, 'a')).toHaveLength(0);
});

test('a Box with no ready time yet says it is starting', async () => {
	const view = await story(TWO_TURNS, { task_boxes: [WORK_BOX({ state: 'starting', ready_ms: null })] });
	const [line] = byClass(view, 't-pl-box') as [El];
	expect(textOf(line)).toBe('▸ Box · acme/clinic-prototype · starting');
});

test('a Box that started before the first message of the page still gets its line', async () => {
	const view = await story(TWO_TURNS, { task_boxes: [WORK_BOX({ started_at: at(-30) })] });
	expect(byClass(view, 't-pl-box')).toHaveLength(1);
});

test('an image the Box left is a card with the picture inline and an Open link, in the turn it was made', async () => {
	const closed = WORK_BOX({ state: 'closed', closed_at: at(11), artifacts: [ARTIFACT('booking.png', true, 9)] });
	const view = await story(TWO_TURNS, { task_boxes: [closed] });
	const [card] = byClass(view, 't-pl-art') as [El];
	const [img] = all(card, 'img') as [El];
	expect(img.props.src).toBe('/account/api/box/artifact?box=clinic%2Fmcp-1a2b3c4d&name=booking.png&version=1');
	expect(textOf(card)).toContain('booking.png');
	const open = all(card, 'a').find((a) => textOf(a) === 'Open') as El;
	expect(open.props.href).toBe(img.props.src as string);
	expect(order(view)).toEqual(['t-pl-you', 't-pl-say', 't-pl-you', 't-pl-box', 't-pl-art', 't-pl-say', 't-pl-fold']);
});

test('a file that is not an image is a card with its name and size and no picture', async () => {
	const closed = WORK_BOX({ state: 'closed', closed_at: at(11), artifacts: [ARTIFACT('notes.txt', false, 9)] });
	const view = await story(TWO_TURNS, { task_boxes: [closed] });
	const [card] = byClass(view, 't-pl-art') as [El];
	expect(all(card, 'img')).toHaveLength(0);
	expect(textOf(card)).toContain('notes.txt');
	expect(textOf(card)).toContain('20 KB');
});

test('the pull request is a card with its number and a link, above the line that names it', async () => {
	const url = 'https://github.com/acme/clinic-prototype/pull/15';
	const timeline = [...TWO_TURNS.slice(0, 4), said(5, 10, `Opened ${url}. I closed the Box.`)];
	const view = await story(timeline, { task_boxes: [WORK_BOX()], pr: { repo: 'acme/clinic-prototype', number: 15, url } });
	const [card] = byClass(view, 't-pl-pr') as [El];
	expect(textOf(card)).toContain('Pull request #15');
	const open = all(card, 'a').find((a) => textOf(a) === 'Open') as El;
	expect(open.props.href).toBe(url);
	expect(order(view)).toEqual(['t-pl-you', 't-pl-say', 't-pl-you', 't-pl-box', 't-pl-pr', 't-pl-say', 't-pl-fold']);
});

test('the pull request card says where the pull request stands when the page has read it', async () => {
	const url = 'https://github.com/acme/clinic-prototype/pull/15';
	const detail = await detailOf(TWO_TURNS, { task_boxes: [WORK_BOX()], pr: { repo: 'acme/clinic-prototype', number: 15, url } });
	const view = PlaceStory({ detail, you: 'Maya', prState: 'open' });
	const [card] = byClass(view, 't-pl-pr') as [El];
	expect(textOf(card)).toContain('open');
});

test('a task with no work Box and no pull request shows neither', async () => {
	const view = await story(TWO_TURNS);
	expect(byClass(view, 't-pl-box')).toHaveLength(0);
	expect(byClass(view, 't-pl-pr')).toHaveLength(0);
	expect(order(view)).toEqual(['t-pl-you', 't-pl-say', 't-pl-you', 't-pl-say', 't-pl-fold']);
});

test('a detail from a controller that sends no task_boxes still parses', async () => {
	const detail = await detailOf(TWO_TURNS);
	expect(detail.task_boxes).toEqual([]);
});
