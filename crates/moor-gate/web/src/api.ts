export interface Secret {
	name: string;
	linked: boolean;
}

export interface Node {
	name: string;
	scope: 'shared' | 'personal';
	runtime: string;
	online: boolean;
	offline_since?: number;
	used: number;
	max_boxes?: number;
	mem_free_mib?: number;
}

export interface Account {
	email: string;
	csrf: string;
	secrets: Secret[];
	machines: { nodes: Node[]; at: number };
	connect: { mcp_url: string | null; server_url: string };
}

/** Thrown when the session ended; the page offers sign-in again. */
export class SignedOut extends Error {}

async function answer(res: Response): Promise<unknown> {
	if (res.status === 401) throw new SignedOut();
	const body = await res.json().catch(() => ({}));
	if (!res.ok) throw new Error(body.error ?? `The gate answered ${res.status}.`);
	return body;
}

export async function load(): Promise<Account> {
	return (await answer(await fetch('/account/api'))) as Account;
}

export async function link(csrf: string, name: string, value: string): Promise<Secret> {
	const res = await fetch(`/account/api/secrets/${encodeURIComponent(name)}`, {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ value }),
	});
	return (await answer(res)) as Secret;
}

export async function signOut(csrf: string): Promise<void> {
	await fetch('/auth/logout', {
		method: 'POST',
		headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
		body: new URLSearchParams({ csrf }),
	});
}
