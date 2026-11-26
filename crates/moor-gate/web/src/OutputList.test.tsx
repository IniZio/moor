import { expect, test } from 'bun:test';

import type { Output } from './api';
import { OutputList } from './PlaceCards';
import { all, textOf } from './testkit';

const out = (title: string, url: string): Output => ({ kind: 'github.comment', url, title, first_at: 1, last_at: 2 });

test('each output is its title as a link that opens in a new tab, then its kind', () => {
	const list = OutputList({ outputs: [out('Evidence comment', 'https://example.com/acme/web/pull/3#c1'), out('Status', 'https://example.com/acme/web/commit/abc')] });
	if (!list) throw new Error('no list');
	const links = all(list, 'a');
	expect(links.map((a) => a.props.href)).toEqual(['https://example.com/acme/web/pull/3#c1', 'https://example.com/acme/web/commit/abc']);
	expect(links.map(textOf)).toEqual(['Evidence comment', 'Status']);
	expect(links.every((a) => a.props.target === '_blank' && a.props.rel === 'noopener')).toBe(true);
	expect(textOf(list)).toContain('github.comment');
});

test('no outputs shows nothing', () => {
	expect(OutputList({ outputs: [] })).toBeNull();
});
