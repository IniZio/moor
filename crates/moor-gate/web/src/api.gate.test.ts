import { afterEach, beforeEach, expect, test } from 'bun:test';

import { boxGet, boxList, gateAnswer } from './api';

type Call = { url: string; init?: RequestInit };
let calls: Call[] = [];
type Json = string | number | boolean | null | Json[] | { [key: string]: Json };
type Reply = { status: number; body: Json };
const ok = (body: Json): Reply => ({ status: 200, body });
let reply = ok({});

beforeEach(() => {
	calls = [];
	globalThis.fetch = Object.assign(
		async (url: string, init?: RequestInit) => {
			calls.push({ url, init });
			return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { 'Content-Type': 'application/json' } });
		},
		{ preconnect: () => {} },
	);
});
afterEach(() => {
	reply = ok({});
});

const wait = {
	gate: 'review',
	kind: 'approve',
	seq: 4,
	try_n: 1,
	why: null,
	handed: ['plan.md', 'after.png'],
	note: 'Ready for you.',
	command: null,
	tail: null,
	missing: [],
	hint: 'Answer yes to pass the gate, or no to fail this try; a note goes to the agent.',
};

const box = (extra: { [key: string]: Json }) => ({
	box: 'web/gated',
	title: 'Gated task',
	state: 'needs_you',
	stale: false,
	line: 'x',
	updated_at: '2026-11-21T10:00:00Z',
	pr: null,
	timeline: [],
	artifacts: [],
	...extra,
});

test('the card of a gate that asks its owner parses, with no approve target and no menu', async () => {
	const needs = { id: 'review-4', kind: 'approve', question: 'Gate review asks your approval.', detail: 'Ready for you.', picks: [], other: null, approve: null, plan: false, gate: wait };
	const gate = { gates: ['review'], status: { state: 'open', gate: 'review', try_n: 1, next: 'ask' } };
	reply = ok(box({ needs, gate }));
	const got = await boxGet('web/gated');
	expect(got.needs?.kind).toBe('approve');
	expect(got.needs?.approve).toBeNull();
	expect(got.needs?.gate?.handed).toEqual(['plan.md', 'after.png']);
	expect(got.gate?.status).toEqual({ state: 'open', gate: 'review', try_n: 1, next: 'ask' });
});

test('the card of a stuck gate parses, with its command, output and missing evidence', async () => {
	const stuck = { gate: 'tests', kind: 'stuck', seq: 7, try_n: null, why: 'out of tries: x', handed: [], note: null, command: "sh -c 'exit 3'", tail: 'boom', missing: ['proof.md'], hint: 'Send the agent a message to reopen the gate with fresh tries.' };
	const needs = { id: 'tests-7', kind: 'stuck', question: 'Gate tests is stuck', detail: "sh -c 'exit 3'", picks: [], other: null, approve: null, plan: false, gate: stuck };
	const gate = { gates: ['tests'], status: { state: 'stuck', gate: 'tests', why: 'out of tries: x' } };
	reply = ok(box({ needs, gate }));
	const got = await boxGet('web/gated');
	expect(got.needs?.kind).toBe('stuck');
	expect(got.needs?.gate).toMatchObject({ command: "sh -c 'exit 3'", tail: 'boom', missing: ['proof.md'], try_n: null });
	expect(got.gate?.status.state).toBe('stuck');
});

test('a Box row carries the gate view when it has gates and none when it has not, and every status parses', async () => {
	const statuses = [
		{ state: 'open', gate: 'a', try_n: 2, next: 'agent' },
		{ state: 'failed', gate: 'a', why: 'the owner said no' },
		{ state: 'passed' },
	];
	reply = ok({ boxes: [...statuses.map((status) => box({ gate: { gates: ['a'], status } })), box({})] });
	const rows = await boxList();
	expect(rows.map((r) => r.gate?.status.state)).toEqual(['open', 'failed', 'passed', undefined]);
});

test('an answer of yes sends the prompt it answers and no note', async () => {
	reply = ok({ answered: true, outcome: 'gate review passed, and every gate has passed' });
	const outcome = await gateAnswer('csrf-1', 'web/gated', 'review-4', true);
	expect(outcome).toBe('gate review passed, and every gate has passed');
	const call = calls[0];
	expect(call?.url).toBe('/account/api/box/answer');
	expect((call?.init?.headers as Record<string, string>)['X-Moor-Csrf']).toBe('csrf-1');
	expect(JSON.parse(String(call?.init?.body))).toEqual({ box: 'web/gated', ask: 'review-4', answer: 'yes' });
});

test('an answer of no sends its note trimmed, and leaves it out when it says nothing', async () => {
	reply = ok({ answered: true });
	await gateAnswer('c', 'web/gated', 'review-4', false, '  use blue  ');
	await gateAnswer('c', 'web/gated', 'review-4', false, '   ');
	const bodies = calls.map((c) => JSON.parse(String(c.init?.body)));
	expect(bodies[0]).toEqual({ box: 'web/gated', ask: 'review-4', answer: 'no', note: 'use blue' });
	expect(bodies[1]).toEqual({ box: 'web/gated', ask: 'review-4', answer: 'no' });
});

test('a refused answer says why', async () => {
	reply = { status: 400, body: { error: 'nothing asks you' } };
	await expect(gateAnswer('c', 'web/gated', 'review-4', true)).rejects.toThrow('nothing asks you');
});
