import type { RunRow } from './api';

/** Runs of one pull request, or of none, newest first as the list gave them. */
export type RunGroup = { pull: string | null; runs: RunRow[] };

/** The runs grouped by pull request, each group where its newest run stands; runs for no pull request form one group. */
export function groupByPull(runs: RunRow[]): RunGroup[] {
	const groups = new Map<string | null, RunRow[]>();
	for (const r of runs) {
		const key = r.pull ?? null;
		const found = groups.get(key);
		if (found) found.push(r);
		else groups.set(key, [r]);
	}
	return [...groups].map(([pull, rs]) => ({ pull, runs: rs }));
}

/** A head commit as a person reads it: its first seven characters. */
export function shortSha(sha: string): string {
	return sha.slice(0, 7);
}
