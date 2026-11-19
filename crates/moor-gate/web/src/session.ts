import { useEffect, useState } from 'preact/hooks';

/** How often activity renews the session at most; the gate keeps an idle one for 12 hours. */
const RENEW_EVERY_MS = 10 * 60 * 1000;

/** Where typed text waits while the person signs in again. */
const DRAFT = 'moor.draft.';

const listeners = new Set<() => void>();

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
			fetch('/auth/renew', { method: 'POST', headers: { 'X-Moor-Csrf': csrf } }).then(
				(res) => res.status === 401 && announceSignedOut(),
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
