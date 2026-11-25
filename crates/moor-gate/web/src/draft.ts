import { type BoxDetail, SignedOut } from './api';

/** The evidence file an agent leaves its plan in. */
export const PLAN_FILE = 'plan.json';

/** The newest plan an agent left in a Box's evidence, as text; null while there is none. */
export async function planOf(detail: BoxDetail): Promise<string | null> {
	const newest = detail.artifacts.filter((a) => a.name === PLAN_FILE).sort((a, b) => b.version - a.version)[0];
	if (!newest) return null;
	const res = await fetch(newest.href);
	if (res.status === 401) throw new SignedOut();
	if (!res.ok) throw new Error(`The plan could not be read: the gate answered ${res.status}.`);
	return res.text();
}
