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

/** The box whose peek panel is open, as the address holds it; none when empty. */
export function peekOf(hash: string): string {
	return queryOf(hash).get('peek') ?? '';
}

/** Opens the peek on `box`, or closes it when empty. Replaces the address, so j and k do not fill Back; the app only listens for hashchange, so it is sent by hand. */
export function setPeek(box: string): void {
	history.replaceState(null, '', withQuery(location.hash, { peek: box }));
	dispatchEvent(new HashChangeEvent('hashchange'));
}

/** How the Tasks home groups its rows. */
export type Grouping = 'state' | 'repo';

/** Whose tasks the Tasks home lists: the person's own, or everyone's to read. */
export type Whose = 'mine' | 'everyone';

export type HomeFilter = { repo: string; grp: Grouping };

/** The Tasks home's filter as the address holds it; an unknown `grp` is the default. */
export function homeFilter(hash: string): HomeFilter {
	const query = queryOf(hash);
	return { repo: query.get('repo') ?? '', grp: query.get('grp') === 'repo' ? 'repo' : 'state' };
}

/** Whose tasks the address asks for; anything but `everyone` is the person's own. */
export function whoseHome(hash: string): Whose {
	return queryOf(hash).get('who') === 'everyone' ? 'everyone' : 'mine';
}
