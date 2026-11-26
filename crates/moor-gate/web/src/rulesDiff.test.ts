import { expect, test } from 'bun:test';

import { diffLines, proposalOf, scrubbed, textLines, unredact } from './rulesDiff';

test('a diff keeps the common lines and marks what was added and taken out, in reading order', () => {
	const prev = ['rules:', '  allow:', '    - GET /user', '    - GET /repos/{repo}'];
	const next = ['rules:', '  allow:', '    - GET /repos/{repo}', '    - POST /repos/{repo}/issues/*/comments'];
	expect(diffLines(prev, next)).toEqual([
		{ text: 'rules:', kind: 'same' },
		{ text: '  allow:', kind: 'same' },
		{ text: '    - GET /user', kind: 'del' },
		{ text: '    - GET /repos/{repo}', kind: 'same' },
		{ text: '    - POST /repos/{repo}/issues/*/comments', kind: 'add' },
	]);
});

test('text lines drop only the closing newline', () => {
	expect(textLines('a\nb\n')).toEqual(['a', 'b']);
	expect(textLines('a\n\n')).toEqual(['a', '']);
});

test('the proposal is the last yaml block, with the words around it as the note', () => {
	const said = 'Added one entry under api.github.com.\n\n```yaml\nrules:\n  - hosts: [a]\n```\nUsed the proxy-rules skill.';
	expect(proposalOf(said)).toEqual({
		note: 'Added one entry under api.github.com. Used the proxy-rules skill.',
		text: 'rules:\n  - hosts: [a]\n',
	});
	expect(proposalOf('I cannot write that as a rule.')).toBeNull();
});

test('a kept line the scrub cut comes back as the saved line; a new one stays cut', () => {
	const base = 'rules:\n  - secret: anthropic_key\n';
	const shown = 'rules:\n  - secret: [redacted]\n  - secret: [redacted]\n';
	const back = unredact(base, shown);
	expect(back).toBe('rules:\n  - secret: anthropic_key\n  - secret: [redacted]\n');
	expect(scrubbed(back)).toBe(true);
	expect(scrubbed(unredact(base, 'rules:\n  - secret: [redacted]\n'))).toBe(false);
});
