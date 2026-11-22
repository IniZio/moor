import { expect, test } from 'bun:test';

import type { BoxDetail, Entry } from './api';
import { Line, Timeline } from './Portal';
import { all, textOf } from './testkit';

const entry = (over: Partial<Entry>): Entry => ({ id: 1, at: '2026-11-21T10:00:00Z', kind: 'steered', class: 'you', text: 'x', ...over });

const handoff = 'Gate `lint` has passed. Gate `review` is now in hand.\n\n- Gate `review`: leave proof.md in .moor/artifacts/, then call report_step with step "review", result ok. Then moor asks the Box\'s owner to approve.';

test("moor's own message shows under the speaker moor as a system note, with gate names as the page quotes them and no raw marks", () => {
	const line = Line({ e: entry({ class: 'moor', text: handoff }), gates: ['lint', 'review', 'ship'] });
	const text = textOf(line);
	expect(textOf(all(line, 'span')[0])).toBe('moor');
	expect(text).not.toContain('You');
	expect(text).not.toContain('`');
	expect(text).not.toContain('- Gate');
	expect(text).toContain('Gate “lint” has passed. Gate “review” is now in hand.');
	expect(text).toContain('Gate “review”: leave proof.md in .moor/artifacts/');
	const row = String(all(line, 'div')[0]?.props.class);
	expect(row).toContain('t-moor');
	expect(row).not.toContain('t-you');
});

test("the owner's note inside a no is quoted as the owner's, apart from moor's words", () => {
	const no = entry({ class: 'moor', text: 'The owner did not approve gate `review`.\n\nFix it, then call report_step again.', quote: 'use the blue header\n\n- keep `it` short' });
	const line = Line({ e: no, gates: ['review'] });
	const [quote] = all(line, 'blockquote');
	expect(textOf(quote)).toBe('use the blue header\n\n- keep `it` short');
	expect(textOf(all(line, 'figcaption')[0])).toBe('The owner wrote');
	const rest = textOf(line).replace(textOf(quote), '');
	expect(rest).toContain('The owner did not approve gate “review”.');
	expect(rest).toContain('Fix it, then call report_step again.');
	expect(textOf(all(line, 'span')[0])).toBe('moor');
});

test('a message the person typed still shows as You, as they wrote it', () => {
	const line = Line({ e: entry({ class: 'you', text: 'Use the `blue` header' }), gates: ['review'] });
	expect(textOf(all(line, 'span')[0])).toBe('You');
	expect(textOf(line)).toContain('Use the `blue` header');
	expect(String(all(line, 'div')[0]?.props.class)).toContain('t-you');
});

test('setup lines fold as setting up the Box on its machine; other routine lines fold as a count', () => {
	const detail = (machine: string | null, kinds: string[]): BoxDetail => ({
		box: 'web/a',
		repo: 'acme/web',
		other_repos: [],
		title: 'Task',
		state: 'working',
		stale: false,
		line: '',
		updated_at: '2026-11-21T10:00:00Z',
		pr: null,
		failed: false,
		retryable: false,
		plan_first: false,
		from_workflow: false,
		artifacts: [],
		machine,
		timeline: kinds.map((kind, i) => entry({ id: i + 1, kind, class: 'routine', text: kind })),
	});
	const summary = (d: BoxDetail) => textOf(all(Timeline({ detail: d }), 'summary')[0]);
	const real = ['started', 'setting_up', 'box_requested', 'step_started', 'box_progress', 'step_finished', 'box_ready', 'cloned', 'working', 'session_started'];
	expect(summary(detail('local-agent', real))).toBe('▸ Set up the Box on local-agent');
	expect(summary(detail(null, real))).toBe('▸ 10 setup notes');
	expect(summary(detail('local-agent', ['step_finished', 'idled']))).toBe('▸ 2 setup notes');
});
