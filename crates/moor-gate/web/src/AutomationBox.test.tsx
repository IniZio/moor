import { expect, test } from 'bun:test';

import type { Workflow } from './api';
import { AutomationBox, type BoxProps, phaseOf } from './AutomationBox';
import { all, button, type El, textOf } from './testkit';

const SAVED = 'Run the check. Post one comment.';

function box(part: Partial<BoxProps>, calls: string[] = []) {
	const flow = { name: 'pr-evidence', prompt: SAVED, repos: ['acme/web'], cron: null, timezone: null, on_pull_request: true, run_as: 'ana@example.com', may: { allow: { 'github-api': ['POST /repos/{repo}/statuses/*'] } }, revision: 3 } as Workflow;
	const note = (what: string) => () => calls.push(what);
	return AutomationBox({
		flow, prompt: SAVED, read: null, showFile: true, busy: false, problem: null, saved: false,
		onInput: note('input'), onRead: note('read'), onSave: note('save'), onDiscard: note('discard'), onReload: note('reload'), onToggle: note('toggle'),
		...part,
	}) as El;
}

const lit = (el: El) => all(el, 'div').filter((d) => String((d.props as { class?: string }).class ?? '').includes('fadd')).map(textOf);

test('a saved prompt shows its file with the config entries as written, and offers no Save', () => {
	const el = box({});
	expect(textOf(el)).toContain('run_as: ana@example.com');
	expect(textOf(el)).toContain('- POST /repos/{repo}/statuses/*');
	expect(button(el, 'Save')).toBeUndefined();
	expect(button(el, 'Read it')).toBeUndefined();
	expect(button(el, 'Hide file')).toBeDefined();
});

test('while writing, the file keeps the saved prompt and Read it is the next step', () => {
	const calls: string[] = [];
	const el = box({ prompt: SAVED + ' Also post to chat.' }, calls);
	expect(phaseOf(SAVED, SAVED + ' x', null)).toBe('writing');
	expect(textOf(el)).not.toContain('Also post to chat.new');
	expect(button(el, 'Save')).toBeUndefined();
	button(el, 'Read it')?.props.onClick?.();
	expect(calls).toEqual(['read']);
});

test('once read, only the changed line is lit and Save is offered', () => {
	const next = SAVED + ' Also post to chat.';
	const el = box({ prompt: next, read: next });
	expect(lit(el)).toEqual(['Also post to chat.new']);
	expect(button(el, 'Save')).toBeDefined();
	expect(button(el, 'Discard')).toBeDefined();
});

test('typing after Read it goes back to writing', () => {
	expect(phaseOf(SAVED, SAVED + ' Then stop.', SAVED + ' Then')).toBe('writing');
});

test('a stale save offers to load the other change', () => {
	const calls: string[] = [];
	const el = box({ prompt: SAVED + ' x', read: SAVED + ' x', problem: { text: 'moved', stale: true } }, calls);
	button(el, 'Discard mine and load theirs')?.props.onClick?.();
	expect(calls).toEqual(['reload']);
});

test('Hide file leaves the prompt alone', () => {
	const el = box({ showFile: false });
	expect(textOf(el)).not.toContain('run_as:');
	expect(button(el, 'Show file')).toBeDefined();
});
