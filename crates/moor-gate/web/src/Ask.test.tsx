import { afterEach, beforeEach, expect, test } from 'bun:test';
import { isValidElement, type VNode } from 'preact';

import { boxGet, boxStart, load } from './api';
import { AskedTask, askedPollMs } from './AskedTask';
import { POLL_MS, Timeline } from './Portal';
import { FailedCard } from './Retry';
import { Composer } from './Steer';
import { TaskHead } from './TaskHead';
import { all, textOf } from './testkit';

type Json = string | number | boolean | null | Json[] | { [key: string]: Json };
let calls: { url: string; body: Json }[] = [];
let replies: Json[] = [];

beforeEach(() => {
	calls = [];
	globalThis.fetch = Object.assign(
		async (url: string, init?: RequestInit) => {
			calls.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null });
			return new Response(JSON.stringify(replies.shift() ?? {}), { status: 200, headers: { 'Content-Type': 'application/json' } });
		},
		{ preconnect: () => {} },
	);
});
afterEach(() => {
	replies = [];
});

const NOW = new Date(2026, 10, 24, 11, 0, 30).getTime();
const ASKED_AT = new Date(2026, 10, 24, 11, 0, 0).toISOString();
const QUESTION = 'Where is GET /items handled?';

/** What the gate sends of a question's task: the claim's repo, words and time, and no work Box behind it. */
const task = (over: { [key: string]: Json } = {}) => ({
	box: 'place-1a2b3c4d',
	repo: 'acme/web',
	other_repos: [],
	title: QUESTION,
	prompt: QUESTION,
	state: 'done',
	stale: false,
	line: 'Idle',
	updated_at: ASKED_AT,
	last_event_at: ASKED_AT,
	task_at: ASKED_AT,
	pr: null,
	timeline: [
		{ id: 1, at: ASKED_AT, kind: 'steered', class: 'you', text: QUESTION },
		{ id: 2, at: ASKED_AT, kind: 'agent_said', class: 'message', text: 'It is `items()` in api/app.py.' },
	],
	artifacts: [],
	...over,
});

test('a person who is on the roster sends, the task opens, and it shows their message and the answer with no work Box', async () => {
	replies = [{ box: 'place-1a2b3c4d' }, task()];
	const name = await boxStart('csrf', { repos: ['acme/web'], prompt: QUESTION, base: '', node: '' });
	expect(name).toBe('place-1a2b3c4d');
	expect(calls[0]).toEqual({ url: '/account/api/boxes', body: { repos: ['acme/web'], prompt: QUESTION, base: '', node: '' } });
	const got = await boxGet(name);
	expect(calls[1]?.url).toBe('/account/api/box?box=place-1a2b3c4d');
	const story = textOf(Timeline({ detail: got }));
	expect(story).toContain(QUESTION);
	expect(story).toContain('api/app.py');
	const head = textOf(TaskHead({ t: got, now: NOW, actions: null }));
	expect(head).toContain('acme/web');
	expect(head).toContain(QUESTION);
	expect(head).not.toMatch(/Box|branch|moor\/|on \w+-\d|last event|asked by/);
});

test('the head of a question\'s task has no branch, machine or event line', () => {
	const head = TaskHead({ t: { ...(task() as never), state: 'done', machine: 'engine-03', branch: 'moor/x', workflow: null }, now: NOW, actions: null, asked: ASKED_AT });
	expect(all(head, 'p')).toHaveLength(0);
	expect(textOf(head)).not.toContain('engine-03');
});

/** The parts the page is made of, in order. */
const parts = (view: VNode) => [view.props.children].flat(3).filter(isValidElement).map((el) => el.type);

test('a question\'s task page is its head, its timeline and the box to write in, with no tabs, prompt, rail or terminal', async () => {
	replies = [task()];
	const detail = await boxGet('place-1a2b3c4d');
	const view = AskedTask({ detail, error: '', now: NOW, csrf: 'c', reload: () => {} });
	expect(parts(view)).toEqual([TaskHead, FailedCard, 'section', Composer]);
});

test('a task page younger than a minute polls fast, and every other page at the usual pace', async () => {
	replies = [task(), task({ task_at: null })];
	const young = await boxGet('place-1a2b3c4d');
	const work = await boxGet('place-1a2b3c4d');
	expect(askedPollMs(young, NOW)).toBeLessThan(POLL_MS);
	expect(askedPollMs(young, NOW + 60_000)).toBe(POLL_MS);
	expect(askedPollMs(work, NOW)).toBe(POLL_MS);
	expect(askedPollMs(null, NOW)).toBe(POLL_MS);
});

const account = (over: { [key: string]: Json } = {}) => ({
	email: 'ann@example.com',
	csrf: 'c',
	secrets: [],
	machines: { nodes: [], at: 0 },
	connect: { mcp_url: null, server_url: 'https://moor.test' },
	...over,
});

test('the account says whether the person asks, and a gate that does not say means they do not', async () => {
	replies = [account({ asks: true }), account()];
	expect((await load()).asks).toBe(true);
	expect((await load()).asks).toBe(false);
});
