import { expect, test } from 'bun:test';

import type { Line, Step } from './api';
import { runWriteUps, writeUpsOf } from './WriteUp';

test('only a markdown file with a link is read inline', () => {
	const evidence = [
		{ name: 'comment.md', href: '/account/api/artifact?box=b&name=comment.md' },
		{ name: 'shot.png', href: '/account/api/artifact?box=b&name=shot.png' },
		{ name: 'notes.MD', href: null },
	];
	expect(writeUpsOf(evidence)).toEqual([{ name: 'comment.md', href: '/account/api/artifact?box=b&name=comment.md' }]);
});

test("a run shows each write-up its steps handed in once, even from a closed step", () => {
	const line = (name: string): Line => ({ at: 1, kind: 'reported', text: 'Reported', evidence: [{ name, href: `/a?name=${name}` }], unanswered: null });
	const step = (history: Line[]): Step => ({ id: 's', title: 's', kind: 'work', state: 'passed', history, expects: [] });
	const run = { steps: [step([line('comment.md'), line('shot.png')]), step([line('comment.md')])], lines: [line('summary.md')] };
	expect(runWriteUps(run).map((w) => w.name)).toEqual(['comment.md', 'summary.md']);
});
