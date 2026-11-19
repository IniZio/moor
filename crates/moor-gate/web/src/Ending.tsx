import { useRef, useState } from 'preact/hooks';

import { type BoxDetail, SignedOut } from './api';
import { boxClose, boxStop, Refused, type Unsaved } from './steerApi';
import './steer.css';

/** The states in which the agent runs, so Stop has something to end. */
const STOPPABLE = ['working', 'needs_you', 'done'];

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function unsavedWords({ ahead, dirty }: Unsaved): string {
	const held = [dirty > 0 && count(dirty, 'uncommitted file', 'uncommitted files'), ahead > 0 && count(ahead, 'unpushed commit', 'unpushed commits')].filter(Boolean);
	return `This task has ${held.join(' and ')}. Push them from the terminal, or discard them and close.`;
}

function whyNot(e: Error): string {
	if (e instanceof SignedOut) return 'Your session ended. Reload to sign in again.';
	return e instanceof Refused && e.unsaved && (e.unsaved.ahead > 0 || e.unsaved.dirty > 0) ? unsavedWords(e.unsaved) : e.message;
}

/** Stop beside the state, and Close under a menu with a confirm; Close removes the Box. */
export function HeaderActions({ detail, csrf, onDone }: { detail: BoxDetail; csrf: string; onDone: () => void }) {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [refused, setRefused] = useState('');
	const dialog = useRef<HTMLDialogElement>(null);
	const menu = useRef<HTMLDivElement>(null);
	const opener = useRef<HTMLButtonElement>(null);
	const closed = detail.state === 'closed';

	const run = async (go: () => Promise<void>, onError: (e: Error) => void) => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await go();
			onDone();
		} catch (e) {
			onError(e as Error);
		} finally {
			setBusy(false);
		}
	};
	const close = (force: boolean) =>
		run(
			async () => {
				await boxClose(csrf, detail.box, force);
				dialog.current?.close();
			},
			(e) => setRefused(whyNot(e)),
		);

	return (
		<div class="t-acts">
			{STOPPABLE.includes(detail.state) && (
				<button type="button" class="btn t-stop" disabled={busy} onClick={() => void run(() => boxStop(csrf, detail.box), (e) => setError(whyNot(e)))}>
					{busy ? 'Stopping…' : 'Stop'}
				</button>
			)}
			{!closed && (
				<>
					<button type="button" class="btn ghost icon" ref={opener} popovertarget="t-more" aria-label="More actions" aria-haspopup="menu" disabled={busy}>
						⋯
					</button>
					<div
						id="t-more"
						ref={menu}
						class="pop t-more"
						popover="auto"
						onToggle={(e) => {
							const at = opener.current?.getBoundingClientRect();
							if (at && e.newState === 'open' && menu.current) {
								menu.current.style.top = `${at.bottom + 4}px`;
								menu.current.style.right = `${document.documentElement.clientWidth - at.right}px`;
							}
						}}
					>
						<button
							type="button"
							class="t-more-item t-more-danger"
							onClick={() => {
								menu.current?.hidePopover();
								setRefused('');
								dialog.current?.showModal();
							}}
						>
							Close task…
						</button>
					</div>
				</>
			)}
			{error && (
				<p class="notice danger t-act-err" role="alert">
					{error}
				</p>
			)}
			<dialog ref={dialog} class="t-confirm" aria-labelledby="t-confirm-h" onClose={() => setRefused('')}>
				<h2 id="t-confirm-h">Close this task?</h2>
				<p class="muted">The Box is removed. Pushed commits and evidence stay.</p>
				{refused && (
					<p class="notice danger" role="alert">
						{refused}
					</p>
				)}
				<div class="actions">
					<button type="button" class="btn t-close-go" disabled={busy} onClick={() => void close(Boolean(refused))}>
						{busy ? 'Closing…' : refused ? 'Discard and close' : 'Close task'}
					</button>
					<button type="button" class="btn ghost" disabled={busy} onClick={() => dialog.current?.close()}>
						Keep it
					</button>
				</div>
			</dialog>
		</div>
	);
}
