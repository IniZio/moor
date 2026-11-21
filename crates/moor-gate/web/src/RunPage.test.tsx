import { expect, test } from 'bun:test';

import type { Step } from './api';
import { StepItem } from './RunPage';
import { all, textOf } from './testkit';

const gate = (over: Partial<Step>): Step => ({
	id: 'done',
	title: 'done',
	kind: 'gate',
	done_when: 'the agent reports posted.md; its checks exit 0',
	command: 'test -f never.txt',
	checks: [{ run: 'test -f never.txt', says: 'The file never.txt exists.' }],
	state: 'passed',
	history: [],
	expects: [],
	...over,
});

test('a gate shows its name once with its state, never "done done", and its entries beneath', () => {
	const row = StepItem({
		step: gate({ history: [{ at: 1, kind: 'entered', text: 'Entered (try 1)', evidence: [] }, { at: 2, kind: 'checked', text: 'Check exited 0 (try 1)', evidence: [] }] }),
		number: 2,
	});
	const [summary] = all(row, 'summary');
	expect(textOf(summary)).toBe('gate2. donepassed');
	expect(textOf(summary)).not.toContain('done done');
	const entries = all(row, 'ul').flatMap((ul) => all(ul, 'li')).map(textOf);
	expect(entries.some((e) => e.endsWith('Entered (try 1)'))).toBe(true);
	expect(entries.some((e) => e.endsWith('Check exited 0 (try 1)'))).toBe(true);
});

test('each gate state reads as a person would say it', () => {
	const word = (state: Step['state']) => textOf(all(StepItem({ step: gate({ state }), number: 1 }), 'summary')[0]).replace('gate1. done', '');
	expect([word('pending'), word('running'), word('waiting'), word('passed'), word('failed')]).toEqual(['not reached', 'in hand', 'waiting for you', 'passed', 'failed']);
});

test('a gate check with a sentence shows it first and the command behind Show the check; one without keeps the command line', () => {
	const row = StepItem({ step: gate({ state: 'waiting' }), number: 2 });
	const text = textOf(row);
	expect(text.indexOf('The file never.txt exists.')).toBeLessThan(text.indexOf('Show the check'));
	expect(all(row, 'code')).toHaveLength(0);
	expect(textOf(all(row, 'pre')[0])).toBe('test -f never.txt');
	const bare = StepItem({ step: gate({ checks: [{ run: 'test -f never.txt', says: null }] }), number: 2 });
	expect(all(bare, 'code').map(textOf)).toEqual(['test -f never.txt']);
});

test('a plan step keeps its words: a passed work step is done', () => {
	const row = StepItem({ step: gate({ kind: 'work', title: 'Audit', checks: null }), number: 1 });
	expect(textOf(all(row, 'summary')[0])).toBe('work1. Auditdone');
});
