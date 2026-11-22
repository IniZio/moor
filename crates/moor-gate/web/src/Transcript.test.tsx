import { expect, test } from 'bun:test';

import type { Entry } from './api';
import { ToolLine } from './Transcript';
import { all, textOf } from './testkit';

const said = 'Gate `review` is not the gate in hand. The gate in hand is `lint`.';
const input = JSON.stringify({ step: 'review', result: 'ok', evidence: ['proof.md'] }, null, 2);

const entry = (tool: Partial<NonNullable<Entry['tool']>>): Entry => ({
	id: 0,
	key: 'k1',
	at: '2026-11-21T10:00:00Z',
	kind: 'tool',
	class: 'step',
	text: 'x',
	tool: { name: 'mcp__moor__report_step', title: 'Used report_step (moor)', failed: false, refused: false, input: { text: input, cut: false }, output: { text: said, cut: false }, ...tool },
});

test('a refused report is a plain item that says moor did not accept it and why, with the JSON folded away', () => {
	const refused = entry({ title: 'moor did not accept the report', refused: true });
	const line = ToolLine({ e: refused, when: '19:09' });
	const text = textOf(line);
	expect(text).toContain('moor did not accept the report');
	expect(text).toContain('Gate review is not the gate in hand. The gate in hand is lint.');
	expect(text).not.toContain('`');
	expect(text).not.toContain('Error');
	expect(text).not.toContain('(failed)');
	const folds = all(line, 'details');
	expect(folds).toHaveLength(1);
	expect(folds[0]?.props.open).toBeFalsy();
	expect(all(line, 'div').some((d) => String(d.props.class ?? '').includes('t-err'))).toBe(false);
});

test('a call that really failed stays red, open and labelled Error', () => {
	const line = ToolLine({ e: entry({ title: 'Used start_box (moor) (failed)', failed: true }), when: '19:09' });
	expect(line?.props.class).toContain('t-tool-err');
	expect(line?.props.open).toBe(true);
	const [summary] = Array.isArray(line?.props.children) ? line.props.children : [];
	expect(textOf(all(summary, 'span')[0])).toBe('Error');
});
