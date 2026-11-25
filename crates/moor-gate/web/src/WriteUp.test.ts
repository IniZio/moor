import { expect, test } from 'bun:test';

import { writeUpsOf } from './WriteUp';

test('only a markdown file with a link is read inline', () => {
	const evidence = [
		{ name: 'comment.md', href: '/account/api/artifact?box=b&name=comment.md' },
		{ name: 'shot.png', href: '/account/api/artifact?box=b&name=shot.png' },
		{ name: 'notes.MD', href: null },
	];
	expect(writeUpsOf(evidence)).toEqual([{ name: 'comment.md', href: '/account/api/artifact?box=b&name=comment.md' }]);
});
