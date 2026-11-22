import type { BoxRow } from './api';
import { reposOf } from './HomeList';

/** A set of repos earlier tasks used together, and how many tasks did. */
export type Together = { repos: string[]; count: number };

const OWNER_NAME = /^[\w.-]+\/[\w.-]+$/;

export const isRepoName = (s: string): boolean => OWNER_NAME.test(s);

const stamp = (t: BoxRow): number => Date.parse(t.last_event_at ?? t.updated_at) || 0;

/** When each repo was last used by one of the person's tasks. */
export function lastUsed(rows: BoxRow[]): Map<string, number> {
	const at = new Map<string, number>();
	for (const t of rows) for (const r of reposOf(t)) at.set(r, Math.max(at.get(r) ?? 0, stamp(t)));
	return at;
}

/** The repos the person has used, recently used first, then the known ones they have not used by name. */
export function recentRepos(rows: BoxRow[], known: string[]): string[] {
	const used = lastUsed(rows);
	return [...new Set([...used.keys(), ...known])].sort((a, b) => (used.get(b) ?? 0) - (used.get(a) ?? 0) || a.localeCompare(b));
}

/** Up to three repo sets of two or more that earlier tasks used, the most often used first. */
export function usedTogether(rows: BoxRow[]): Together[] {
	const by = new Map<string, Together & { at: number }>();
	for (const t of rows) {
		const repos = reposOf(t);
		if (repos.length < 2) continue;
		const key = repos.join(',');
		const seen = by.get(key);
		by.set(key, { repos, count: (seen?.count ?? 0) + 1, at: Math.max(seen?.at ?? 0, stamp(t)) });
	}
	return [...by.values()]
		.sort((a, b) => b.count - a.count || b.at - a.at)
		.slice(0, 3)
		.map(({ repos, count }) => ({ repos, count }));
}
