import { afterEach, beforeEach, expect, test } from 'bun:test';

import { boxGet, boxList, gateAnswer, runGet, workflowGet, workflowList } from './api';
import { LOST, NOT_RUN } from './testkit';

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

test('a stage moor got no answer from carries its cause on the card, the status and the run page line', async () => {
	const stuck = { gate: 'tests', kind: 'stuck', seq: 7, try_n: 1, why: 'w', handed: [], note: null, command: 'make', tail: null, missing: [], unanswered: NOT_RUN, hint: 'h' };
	const needs = { id: 'tests-7', kind: 'stuck', question: 'Gate tests is stuck', detail: null, picks: [], other: null, approve: null, plan: false, gate: stuck };
	reply = ok(box({ needs, gate: { gates: ['tests'], status: { state: 'stuck', gate: 'tests', why: 'w', unanswered: NOT_RUN } } }));
	const got = await boxGet('web/gated');
	expect(got.needs?.gate?.unanswered).toEqual(NOT_RUN);
	expect(got.gate?.status).toMatchObject({ unanswered: NOT_RUN });
	const line = { at: 2, kind: 'gate_waiting', text: 'Waiting for a person: w', evidence: [], unanswered: LOST };
	reply = ok({ workflow: 'evidence', id: 3, due_at: 1, phase: 'waiting', status: 'waiting', label: 'waiting', steps: [], lines: [line] });
	expect((await runGet('evidence', 3)).lines[0]?.unanswered).toEqual(LOST);
});

test('the gate view carries the note of the last accepted report, or none', async () => {
	const status = { state: 'passed' };
	reply = ok({ boxes: [box({ gate: { gates: ['a'], status, note: 'Typo fixed.' } }), box({ gate: { gates: ['a'], status, note: null } }), box({ gate: { gates: ['a'], status } })] });
	const rows = await boxList();
	expect(rows.map((r) => r.gate?.note)).toEqual(['Typo fixed.', null, undefined]);
});

test('the words a person reads carry no code marks, and a gate is named as the page names it', async () => {
	const wait = { gate: 'review', kind: 'approve', seq: 4, try_n: 1, why: null, handed: [], note: 'Wrote `proof.md`.', command: null, tail: null, missing: [], hint: 'Answer for `review`.' };
	const needs = { id: 'review-4', kind: 'approve', question: 'Gate `review` asks your approval.', detail: 'Handed in `proof.md`', picks: [], other: null, approve: null, plan: false, gate: wait };
	const gate = { gates: ['lint', 'review'], status: { state: 'open', gate: 'review', try_n: 1, next: 'ask' } };
	reply = ok({ boxes: [box({ needs, gate, line: 'Waiting for `review` and `proof.md`' })] });
	const [row] = await boxList();
	expect(row?.line).toBe('Waiting for “review” and proof.md');
	expect(row?.needs?.question).toBe('Gate “review” asks your approval.');
	expect(row?.needs?.detail).toBe('Handed in proof.md');
	expect(row?.needs?.gate?.hint).toBe('Answer for “review”.');
	expect(row?.needs?.gate?.note).toBe('Wrote proof.md.');
	reply = ok(box({ needs, gate, line: 'Gate `review` asks' }));
	expect((await boxGet('web/gated')).line).toBe('Gate “review” asks');
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

test("a timeline entry of class moor parses, with the owner's quote when moor carries one", async () => {
	const at = '2026-11-21T10:00:00Z';
	const timeline = [
		{ id: 1, at, kind: 'steered', class: 'you', text: 'Fix it' },
		{ id: 2, at, kind: 'steered', class: 'moor', text: 'The owner did not approve gate review.', quote: 'blue header' },
	];
	reply = ok(box({ timeline }));
	const got = await boxGet('web/gated');
	expect(got.timeline.map((e) => e.class)).toEqual(['you', 'moor']);
	expect(got.timeline[1]?.quote).toBe('blue header');
});

const verdict = { name: 'check failed', tone: 'bad', says: 'The pull request check failed.' };

test('the list row, the run row and the run page keep the outcome the server named', async () => {
	const flow = { name: 'evidence', repos: ['acme/web'], enabled: true, has_plan: false, plan_stale: false };
	reply = ok({ workflows: [{ ...flow, last: { status: 'success', label: 'check failed', verdict, due_at: 1 } }] });
	expect((await workflowList())[0]?.last?.verdict).toEqual(verdict);
	const outcomes = [{ name: 'nothing to do', tone: 'plain', anytime: true, checks: ['true'] }, { name: 'done' }];
	const row = { id: 3, due_at: 1, trigger: 'manual', status: 'success', label: 'check failed', verdict };
	reply = ok({ ...flow, timeout_minutes: 90, prompt: 'p', plan_edited: false, revision: 1, outcomes, runs: [row] });
	const page = await workflowGet('evidence');
	expect(page.runs[0]?.verdict?.name).toBe('check failed');
	expect(page.outcomes?.map((o) => o.name)).toEqual(['nothing to do', 'done']);
	reply = ok({ workflow: 'evidence', id: 3, due_at: 1, phase: 'running', status: 'running', label: 'running', closing: 'deciding', steps: [], lines: [] });
	expect((await runGet('evidence', 3)).closing).toBe('deciding');
	reply = ok({ workflow: 'evidence', id: 3, due_at: 1, phase: 'done', status: 'success', label: 'check failed', verdict, steps: [], lines: [] });
	const run = await runGet('evidence', 3);
	expect([run.verdict?.tone, run.closing ?? null]).toEqual(['bad', null]);
});
