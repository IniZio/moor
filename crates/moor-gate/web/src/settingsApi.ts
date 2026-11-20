import * as v from 'valibot';

import { SignedOut } from './api';

const FailureSchema = v.object({ error: v.string() });

/** The gate's answer parsed as `schema`, or why it is not one. */
async function answer<T extends v.GenericSchema>(res: Response, schema: T): Promise<v.InferOutput<T>> {
	if (res.status === 401) throw new SignedOut();
	const body: unknown = await res.json().catch(() => null);
	if (!res.ok) {
		const failure = v.safeParse(FailureSchema, body);
		throw new Error(failure.success ? failure.output.error : `The gate answered ${res.status}.`);
	}
	return v.parse(schema, body);
}

function send(csrf: string, method: string, path: string, body?: Record<string, string>) {
	return fetch(path, {
		method,
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(body ?? {}),
	});
}

const TokenSchema = v.object({ token: v.string(), expires_in: v.number() });

/** A fresh one-use token that enrolls a machine as the signed-in person; an earlier one stops working. */
export async function machineToken(csrf: string) {
	return answer(await send(csrf, 'POST', '/account/api/machines/token'), TokenSchema);
}

/** Removes one of the signed-in person's machines. */
export async function machineRemove(csrf: string, name: string) {
	const path = `/account/api/machines/${encodeURIComponent(name)}`;
	return answer(await send(csrf, 'DELETE', path), v.object({ name: v.string(), changed: v.boolean() }));
}

const StepSchema = v.picklist(['resolve', 'manifest', 'fetch', 'install', 'setup']);

const InstallsSchema = v.object({
	model: v.nullish(v.string()),
	marketplaces: v.array(v.object({ name: v.string(), repo: v.string() })),
	plugins: v.array(v.string()),
});

const CheckSchema = v.object({
	/** `https://github.com/OWNER/NAME[#REF]`, as it was set. */
	source: v.nullish(v.string()),
	resolved: v.nullish(
		v.object({
			sha: v.string(),
			setup: v.boolean(),
			claude: InstallsSchema,
			codex: InstallsSchema,
			unresolved: v.optional(v.array(v.string()), []),
		}),
	),
	failures: v.array(v.object({ step: StepSchema, detail: v.string() })),
	last_failed: v.nullish(v.object({ step: StepSchema, detail: v.string(), box_name: v.string(), at_ms: v.number() })),
});

export type ProfileCheck = v.InferOutput<typeof CheckSchema>;
export type Installs = v.InferOutput<typeof InstallsSchema>;

/** The stored profile, checked against GitHub now, with the last failure any Box had applying it. */
export async function profileGet(): Promise<ProfileCheck> {
	return answer(await fetch('/account/api/profile'), CheckSchema);
}

/** What `source` would install, without saving it. */
export async function profileCheck(csrf: string, source: string): Promise<ProfileCheck> {
	return answer(await send(csrf, 'POST', '/account/api/profile/check', { source }), CheckSchema);
}

/** Saves `source` as the profile once it checks clean; a failing check is refused with why. */
export async function profileSet(csrf: string, source: string): Promise<ProfileCheck> {
	return answer(await send(csrf, 'PUT', '/account/api/profile', { source }), CheckSchema);
}

export async function profileRemove(csrf: string): Promise<void> {
	await answer(await send(csrf, 'DELETE', '/account/api/profile'), v.unknown());
}

/** A profile source as typed, checked for shape before the gate is asked. */
export const SourceInput = v.pipe(
	v.string(),
	v.trim(),
	v.nonEmpty('Paste the URL of a GitHub repo.'),
	v.regex(/^https:\/\/github\.com\/[\w.-]+\/[\w.-]+?(\.git)?\/?(#[^\s#]+)?$/, 'Use https://github.com/OWNER/NAME, then #ref to pin a branch, tag or commit.'),
);

const GitHubAppSchema = v.nullable(v.object({ install_url: v.string(), repos: v.array(v.string()) }));

/** The operator's GitHub App: its install page and the repos it is installed on; null when moor has none. */
export async function githubApp() {
	return answer(await fetch('/account/api/github-app'), GitHubAppSchema);
}
