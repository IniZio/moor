import { expect, test } from 'bun:test';

import { homeFilter, pathOf, queryOf, withQuery } from './hashQuery';
import { sectionAt } from './Settings';
import { workflowsAt } from './Workflows';

test('a filter written into the address reads back the same, slash kept readable', () => {
	const hash = withQuery('#/', { repo: 'acme/web', grp: 'repo' });
	expect(hash).toBe('#/?repo=acme/web&grp=repo');
	expect(homeFilter(hash)).toEqual({ repo: 'acme/web', grp: 'repo' });
	expect(homeFilter('#/')).toEqual({ repo: '', grp: 'state' });
	expect(homeFilter('#/?grp=nonsense')).toEqual({ repo: '', grp: 'state' });
});

test('clearing one parameter keeps the others and drops the question mark when none is left', () => {
	const both = withQuery('#/', { repo: 'acme/web', grp: 'repo' });
	expect(withQuery(both, { repo: '' })).toBe('#/?grp=repo');
	expect(withQuery(withQuery(both, { repo: '' }), { grp: '' })).toBe('#/');
	expect(queryOf(withQuery('#/?x=1', { repo: 'a/b' })).get('x')).toBe('1');
});

test('an empty address becomes the home address', () => {
	expect(withQuery('', { grp: 'repo' })).toBe('#/?grp=repo');
});

test('the other routes read as they did, with or without a filter after them', () => {
	for (const route of ['#/box/a%2Fb/evidence', '#/box/a%2Fb', '#/new', '#/workflows', '#/workflows/ci/runs/3', '#/settings', '#/settings/machines', '#/repos/acme/web']) {
		expect(pathOf(route)).toBe(route);
		expect(pathOf(withQuery(route, { repo: 'acme/web' }))).toBe(route);
	}
	expect(pathOf('#/box/a%2Fb/evidence?repo=acme/web')).toBe('#/box/a%2Fb/evidence');
	expect(workflowsAt(pathOf('#/workflows/ci/runs/3?grp=repo'))).toEqual({ page: 'run', name: 'ci', id: 3 });
	expect(sectionAt(pathOf('#/settings/machines?repo=a/b'))).toBe('machines');
});
