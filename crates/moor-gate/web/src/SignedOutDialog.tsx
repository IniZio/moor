import { useEffect, useRef } from 'preact/hooks';

import { onSignedOut, signInHref } from './session';
import './session.css';

/** Asks the person to sign in again when the session ends, keeping the page and what they typed underneath. Once put off, it stays closed on this page. */
export function SignedOutDialog() {
	const dialog = useRef<HTMLDialogElement>(null);
	const putOff = useRef(false);
	useEffect(
		() =>
			onSignedOut(() => {
				const d = dialog.current;
				if (d && !d.open && !putOff.current) d.showModal();
			}),
		[],
	);
	return (
		<dialog ref={dialog} class="signed-out" aria-labelledby="signed-out-h" onClose={() => (putOff.current = true)}>
			<h2 id="signed-out-h">Your session ended</h2>
			<p>Sign in again to keep working. You come back to this page, and what you typed is kept.</p>
			<div class="signed-out-btns">
				<button type="button" class="btn ghost" onClick={() => dialog.current?.close()}>
					Not now
				</button>
				<a class="btn primary" href="/auth/login" onClick={(e) => (e.currentTarget.href = signInHref())}>
					Sign in again
				</a>
			</div>
		</dialog>
	);
}
