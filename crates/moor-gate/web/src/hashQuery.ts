/** The address without its query: `#/box/a%2Fb?repo=x` is `#/box/a%2Fb`, so every route reads as it did before filters. */
export function pathOf(hash: string): string {
	const at = hash.indexOf('?');
	return at < 0 ? hash : hash.slice(0, at);
}

/** The parameters after the `?` of an address. */
export function queryOf(hash: string): URLSearchParams {
	const at = hash.indexOf('?');
	return new URLSearchParams(at < 0 ? '' : hash.slice(at + 1));
}

/** The address with each named parameter set, or removed when its value is empty; other parameters stay. */
export function withQuery(hash: string, changes: Record<string, string>): string {
	const query = queryOf(hash);
	for (const [key, value] of Object.entries(changes)) {
		if (value) query.set(key, value);
		else query.delete(key);
	}
	// Repo names are `owner/name`: keep the slash readable in the address bar.
	const text = query.toString().replace(/%2F/gi, '/');
	const path = pathOf(hash);
	return (path.startsWith('#') ? path : `#/${path}`) + (text ? `?${text}` : '');
}

/** How the Tasks home groups its rows. */
export type Grouping = 'state' | 'repo';

export type HomeFilter = { repo: string; grp: Grouping };

/** The Tasks home's filter as the address holds it; an unknown `grp` is the default. */
export function homeFilter(hash: string): HomeFilter {
	const query = queryOf(hash);
	return { repo: query.get('repo') ?? '', grp: query.get('grp') === 'repo' ? 'repo' : 'state' };
}
