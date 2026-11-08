import * as v from 'valibot';

const SecretSchema = v.object({ name: v.string(), linked: v.boolean() });

const NodeSchema = v.object({
	name: v.string(),
	scope: v.picklist(['shared', 'personal']),
	runtime: v.string(),
	online: v.boolean(),
	offline_since: v.optional(v.number()),
	used: v.number(),
	max_boxes: v.optional(v.number()),
	mem_free_mib: v.optional(v.number()),
});

const AccountSchema = v.object({
	email: v.string(),
	csrf: v.string(),
	secrets: v.array(SecretSchema),
	machines: v.object({ nodes: v.array(NodeSchema), at: v.number() }),
	connect: v.object({ mcp_url: v.nullable(v.string()), server_url: v.string() }),
});

const FailureSchema = v.object({ error: v.string() });

export type Secret = v.InferOutput<typeof SecretSchema>;
export type Node = v.InferOutput<typeof NodeSchema>;
export type Account = v.InferOutput<typeof AccountSchema>;

/** Thrown when the session ended; the page offers sign-in again. */
export class SignedOut extends Error {}

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
