import * as v from 'valibot';

const SecretSchema = v.object({ name: v.string(), linked: v.boolean() });

const NodeSchema = v.object({
	name: v.string(),
	scope: v.picklist(['shared', 'personal']),
	runtime: v.string(),
	online: v.boolean(),
	offline_since: v.optional(v.number()),
	provides: v.optional(v.string()),
	used: v.number(),
	waiting: v.number(),
	max_boxes: v.optional(v.number()),
	mem_free_mib: v.optional(v.number()),
	mem_psi_centi: v.optional(v.number()),
	mem_report_age_secs: v.optional(v.number()),
	pressure_since: v.optional(v.number()),
	box_memory_mib: v.optional(v.number()),
	labels: v.array(v.string()),
	revision: v.optional(v.number(), 0),
	editable: v.optional(v.boolean(), false),
});

const AccountSchema = v.object({
	email: v.string(),
	csrf: v.string(),
	secrets: v.array(SecretSchema),
	machines: v.object({ nodes: v.array(NodeSchema), at: v.number() }),
	connect: v.object({ mcp_url: v.nullable(v.string()), server_url: v.string() }),
});

const RepoConfigSchema = v.object({
	repo: v.string(),
	text: v.nullish(v.string()),
	revision: v.number(),
	edited_by: v.nullish(v.string()),
	edited_at: v.nullish(v.string()),
	can_edit: v.boolean(),
});

const NodeSettingsSchema = v.object({
	name: v.string(),
	max_boxes: v.optional(v.number()),
	labels: v.array(v.string()),
	revision: v.number(),
	edited_by: v.nullish(v.string()),
	edited_at: v.nullish(v.string()),
});

const RepoListSchema = v.object({ repos: v.array(v.string()) });

const FailureSchema = v.object({ error: v.string() });

export type Secret = v.InferOutput<typeof SecretSchema>;
export type Node = v.InferOutput<typeof NodeSchema>;
export type RepoConfig = v.InferOutput<typeof RepoConfigSchema>;
export type NodeSettings = v.InferOutput<typeof NodeSettingsSchema>;
export type Account = v.InferOutput<typeof AccountSchema>;

/** Thrown when the session ended; the page offers sign-in again. */
export class SignedOut extends Error {}

/** Thrown when a save was made against a revision that is no longer the stored one. */
export class Conflict extends Error {}

/** The gate's answer parsed as `schema`, or why it is not one. */
async function answer<T extends v.GenericSchema>(res: Response, schema: T): Promise<v.InferOutput<T>> {
	if (res.status === 401) throw new SignedOut();
	const body: unknown = await res.json().catch(() => null);
	if (!res.ok) {
		const failure = v.safeParse(FailureSchema, body);
		const why = failure.success ? failure.output.error : `The gate answered ${res.status}.`;
		throw res.status === 409 ? new Conflict(why) : new Error(why);
	}
	return v.parse(schema, body);
}

export async function load(): Promise<Account> {
	return answer(await fetch('/account/api'), AccountSchema);
}

export async function link(csrf: string, name: string, value: string): Promise<Secret> {
	const res = await fetch(`/account/api/secrets/${encodeURIComponent(name)}`, {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ value }),
	});
	return answer(res, SecretSchema);
}

export async function signOut(csrf: string): Promise<void> {
	await fetch('/auth/logout', {
		method: 'POST',
		headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
		body: new URLSearchParams({ csrf }),
	});
}

/** `owner/name` as a URL path, each part encoded. */
function repoPath(repo: string): string {
	return `/account/api/repos/${repo.split('/').map(encodeURIComponent).join('/')}`;
}

export async function repoList(): Promise<string[]> {
	return (await answer(await fetch('/account/api/repos'), RepoListSchema)).repos;
}

export async function repoGet(repo: string): Promise<RepoConfig> {
	return answer(await fetch(repoPath(repo)), RepoConfigSchema);
}

/** Replaces the repo's config, against the revision the page read. */
export async function repoSet(csrf: string, repo: string, text: string, revision: number): Promise<RepoConfig> {
	const res = await fetch(repoPath(repo), {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ text, revision }),
	});
	return answer(res, RepoConfigSchema);
}

/** Replaces a node's cap (none for no cap) and labels, against the revision the page read. */
export async function nodeSet(
	csrf: string,
	name: string,
	maxBoxes: number | null,
	labels: string[],
	revision: number,
): Promise<NodeSettings> {
	const res = await fetch(`/account/api/nodes/${encodeURIComponent(name)}`, {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ max_boxes: maxBoxes, labels, revision }),
	});
	return answer(res, NodeSettingsSchema);
}
