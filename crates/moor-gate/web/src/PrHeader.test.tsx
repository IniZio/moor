import { expect, test } from 'bun:test';

import type { RepoPull } from './api';
import { PrInline } from './PrHeader';
import { all, textOf } from './testkit';

const pull = (over: Partial<RepoPull>): RepoPull => ({ repo: 'acme/web', state: 'open', number: 12, checks: null, url: 'https://example.test/pr/12', ...over });
const named = { repo: 'acme/web', number: 12, url: 'https://example.test/pr/12' };
const inline = (pulls: RepoPull[] | null, over: { error?: string; named?: typeof named | null; listed?: number | null } = {}) =>
	PrInline({ pulls, error: over.error ?? '', named: over.named === undefined ? named : over.named, listed: over.listed === undefined ? 12 : over.listed });
const links = (el: ReturnType<typeof PrInline>) => all(el, 'a').map((a) => [textOf(a), a.props.href, a.props.class]);

test('an open pull request the head already links shows its state and checks, and no second link', () => {
	const el = inline([pull({ checks: 'failing' })]);
	expect(textOf(el)).toBe('● open✕ checks failing');
	expect(links(el)).toEqual([]);
});

test('a pushed branch with no pull request yet offers to open one, as the step to take', () => {
	const el = inline([pull({ state: 'none', number: null, url: 'https://example.test/compare' })], { named: null, listed: null });
	expect(links(el)).toEqual([['Open PR ↗', 'https://example.test/compare', 'pr-step pr-go']]);
});

test('a pull request the agent did not name links to itself, and a second repo follows with its own', () => {
	const el = inline([pull({ number: 13, url: 'https://example.test/pr/13' }), pull({ repo: 'acme/api', state: 'merged', number: 4, url: 'https://example.test/pr/4' })], { named: null, listed: null });
	expect(textOf(el)).toContain('acme/api');
	expect(links(el)).toEqual([
		['View PR ↗', 'https://example.test/pr/13', 'pr-step'],
		['Merged ↗', 'https://example.test/pr/4', 'pr-step'],
	]);
});

test('GitHub that could not be read says so in place of the state', () => {
	expect(textOf(inline([pull({ unread: 'rate limited' })]))).toContain('GitHub could not be read: rate limited');
	expect(textOf(inline(null, { error: 'The gate answered 502.' }))).toBe('The gate answered 502.');
	expect(inline(null)).toBeNull();
});
