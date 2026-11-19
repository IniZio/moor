import { useEffect, useState } from 'preact/hooks';

/** How often activity renews the session at most; the gate keeps an idle one for 12 hours. */
const RENEW_EVERY_MS = 10 * 60 * 1000;

/** Where typed text waits while the person signs in again. */
const DRAFT = 'moor.draft.';

const listeners = new Set<() => void>();

/** Set once the gate refused a check, so the page stops asking until it reloads. */
let refused = false;

/** Runs `fn` alone across this site's tabs: they share one cookie, and its refresh token works once. */
function alone<T>(fn: () => Promise<T>): Promise<T> {
	return navigator.locks ? navigator.locks.request('moor-session', fn) : fn();
}

const plainFetch = window.fetch.bind(window);

/**
 * Every request the page makes: when the gate answers `401` because the session's check with the
 * issuer is due, the page checks once and sends the request again. A request answered `401`
 * never ran, so sending it again is safe.
 */
window.fetch = async (input, init) => {
	const res = await plainFetch(input, init);
	const path = new URL(input instanceof Request ? input.url : input, location.href).pathname;
	if (res.status !== 401 || refused || path.startsWith('/auth/')) return res;
	return alone(async () => {
		const again = await plainFetch(input, init);
		if (again.status !== 401 || refused) return again;
		const checked = await plainFetch('/auth/check', { method: 'POST' });
		if (!checked.ok) {
			refused = true;
			return again;
		}
		return plainFetch(input, init);
	});
};

/** Tells the page the session ended, once per ending. */
export function announceSignedOut() {
	for (const fn of listeners) fn();
}

/** Calls `fn` whenever a request finds the session ended. */
export function onSignedOut(fn: () => void): () => void {
	listeners.add(fn);
	return () => listeners.delete(fn);
}

/** The sign-in link that lands back on this page, the part after `#` included. */
export function signInHref(): string {
	return `/auth/login?return=${encodeURIComponent(location.pathname + location.search + location.hash)}`;
}

/** Renews the session now and then whenever the person acts or comes back to the tab, at most every ten minutes. */
export function useRenewWhileActive(csrf: string) {
	useEffect(() => {
		let last = 0;
		const renew = () => {
			if (document.hidden || Date.now() - last < RENEW_EVERY_MS) return;
			last = Date.now();
			void alone(() => plainFetch('/auth/renew', { method: 'POST', headers: { 'X-Moor-Csrf': csrf } })).then(
				(res) => {
					if (res.status !== 401) return;
					refused = true;
					announceSignedOut();
				},
				() => undefined,
			);
		};
		renew();
		const events = ['pointerdown', 'keydown', 'visibilitychange'] as const;
		for (const e of events) document.addEventListener(e, renew, { passive: true });
		return () => {
			for (const e of events) document.removeEventListener(e, renew);
		};
	}, [csrf]);
}

/**
 * Text the person types that outlives a sign-in round trip: kept in this tab's sessionStorage under `key`.
 * `initial`, when given, wins over a kept draft. The third value forgets the draft once it is sent.
 */
export function useDraft(key: string, initial?: string): [string, (text: string) => void, () => void] {
	const name = DRAFT + key;
	const [text, setText] = useState(() => initial ?? sessionStorage.getItem(name) ?? '');
	const set = (next: string) => {
		setText(next);
		if (next) sessionStorage.setItem(name, next);
		else sessionStorage.removeItem(name);
	};
	return [text, set, () => set('')];
}

/** Forgets every kept draft, for a deliberate sign-out. */
export function dropDrafts() {
	for (const k of Object.keys(sessionStorage)) if (k.startsWith(DRAFT)) sessionStorage.removeItem(k);
}
