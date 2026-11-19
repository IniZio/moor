import * as v from 'valibot';

import { type BoxDetail, SignedOut } from './api';

/** The evidence file an agent leaves its plan in. */
export const PLAN_FILE = 'plan.json';

const Failure = v.object({ error: v.string() });

async function post<T extends v.GenericSchema>(csrf: string, path: string, body: Record<string, string | string[] | boolean>, schema: T): Promise<v.InferOutput<T>> {
	const res = await fetch(path, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(body),
	});
	if (res.status === 401) throw new SignedOut();
	const read: unknown = await res.json().catch(() => null);
	if (!res.ok) {
		const failure = v.safeParse(Failure, read);
		throw new Error(failure.success ? failure.output.error : `The gate answered ${res.status}.`);
	}
	return v.parse(schema, read);
}

/** Starts a short Box on `repos` that drafts a plan for `prompt`; answers its name. */
export async function planDraft(csrf: string, repos: string[], prompt: string): Promise<string> {
	return (await post(csrf, '/account/api/plan/draft', { repos, prompt }, v.object({ box: v.string() }))).box;
}

/** Closes a Box the signed-in person started. */
export async function boxClose(csrf: string, box: string): Promise<void> {
	await post(csrf, '/account/api/box/close', { box, force: true }, v.looseObject({}));
}

/** The newest plan an agent left in a Box's evidence, as text; null while there is none. */
export async function planOf(detail: BoxDetail): Promise<string | null> {
	const newest = detail.artifacts.filter((a) => a.name === PLAN_FILE).sort((a, b) => b.version - a.version)[0];
	if (!newest) return null;
	const res = await fetch(newest.href);
	if (res.status === 401) throw new SignedOut();
	if (!res.ok) throw new Error(`The plan could not be read: the gate answered ${res.status}.`);
	return res.text();
}
