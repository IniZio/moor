import { expect, test } from 'bun:test';

import type { Workflow } from './api';
import { GatesNote, PlanView } from './PlanView';
import { button, type El, textOf } from './testkit';

const PLAN = JSON.stringify({
	steps: [
		{ id: 'ask', title: 'Ask the owner', kind: 'approve', question: 'Ship it?', ok: 'ship' },
		{ id: 'ship', title: 'Ship', kind: 'deliver', instructions: 'open the PR', done_when: 'the PR exists' },
	],
});

function flow(part: Partial<Workflow>): Workflow {
	return { cron: null, plan: null, plan_stale: false, plan_making: null, ...part } as Workflow;
}

function view(part: Partial<Workflow>, again = () => {}) {
	return PlanView({ flow: flow(part), busy: false, onAgain: again }) as El;
}

test('the plan is shown read-only, with no way to edit a step', () => {
	const el = view({ plan: PLAN });
	expect(textOf(el)).toContain('Ask the owner');
	expect(textOf(el)).toContain("moor's current reading of the prompt");
	expect(button(el, 'Make it again')).toBeDefined();
	expect(button(el, 'Save')).toBeUndefined();
});

test('a scheduled plan with a step that waits for a person says a run can stop', () => {
	expect(textOf(view({ plan: PLAN, cron: '0 9 * * *' }))).toContain('a scheduled run can stop until you answer');
	expect(textOf(view({ plan: PLAN }))).not.toContain('a scheduled run can stop');
});

test('while moor makes the plan the page says so and links its Box', () => {
	const el = view({ plan_making: { prompt: 'x', box_name: 'daily/w1-plan-ab', since: 1, failed: null } });
	expect(textOf(el)).toContain('moor is making the plan');
	expect(button(el, 'Make it again')).toBeUndefined();
});

test('a plan moor could not make shows why and a way to try again', () => {
	let asked = 0;
	const el = view({ plan_making: { prompt: 'x', box_name: null, since: null, failed: 'the Box finished without leaving a plan' } }, () => asked++);
	expect(textOf(el)).toContain('the Box finished without leaving a plan');
	button(el, 'Make the plan again')?.props.onClick?.();
	expect(asked).toBe(1);
});

const GATES = [
	{ name: 'proof', evidence: ['pr-*'] },
	{ name: 'done', evidence: ['posted.md'], checks: [{ run: 'true', says: 'The pull request has one evidence comment.' }] },
];

function gates(part: Partial<Workflow>, again = () => {}) {
	return GatesNote({ flow: flow({ gates: GATES, ...part }), busy: false, onAgain: again }) as El;
}

test("moor's gates are shown read-only as its reading, with a way to make them again", () => {
	let asked = 0;
	const el = gates({}, () => asked++);
	expect(textOf(el)).toContain("moor's current reading of the prompt");
	expect(textOf(el)).not.toContain('A person wrote these gates');
	button(el, 'Make it again')?.props.onClick?.();
	expect(asked).toBe(1);
	expect(button(el, 'Save')).toBeUndefined();
});

test('gates a person wrote say so and that making them again replaces them', () => {
	expect(textOf(gates({ gates_by: 'person' }))).toContain('A person wrote these gates');
});

test('stale gates say they still hold, and while moor reads the prompt the page says so', () => {
	expect(textOf(gates({ plan_stale: true }))).toContain('They still hold until moor makes them again');
	const el = gates({ plan_making: { prompt: 'x', box_name: 'daily/w1-plan-ab', since: 1, failed: null } });
	expect(textOf(el)).toContain('moor is making the plan');
	expect(button(el, 'Make it again')).toBeUndefined();
});
