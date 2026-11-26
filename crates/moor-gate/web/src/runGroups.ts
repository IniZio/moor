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

/** Runs due on one local day, `Tue 24 Nov`, newest first as the list gave them. */
export type DayGroup = { day: string; runs: RunRow[] };

const DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const two = (n: number) => String(n).padStart(2, '0');

/** A local day as a group head reads it: `Tue 24 Nov`. */
export function dayText(ms: number): string {
	const d = new Date(ms);
	return `${DAYS[d.getDay()]} ${d.getDate()} ${MONTHS[d.getMonth()]}`;
}

/** A local time as a run row reads it: `24 Nov 09:00`. */
export function rowTime(ms: number): string {
	const d = new Date(ms);
	return `${d.getDate()} ${MONTHS[d.getMonth()]} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

/** The runs grouped by the local day each was due. */
export function groupByDay(runs: RunRow[]): DayGroup[] {
	const groups = new Map<string, RunRow[]>();
	for (const r of runs) {
		const day = dayText(r.due_at);
		const found = groups.get(day);
		if (found) found.push(r);
		else groups.set(day, [r]);
	}
	return [...groups].map(([day, rs]) => ({ day, runs: rs }));
}

/** A head commit as a person reads it: its first seven characters. */
export function shortSha(sha: string): string {
	return sha.slice(0, 7);
}
