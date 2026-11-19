import { useEffect, useRef, useState } from 'preact/hooks';

import { type Node, SignedOut } from './api';
import { Command } from './Connect';
import { machineToken } from './settingsApi';

const INSTALL = 'curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh -s -- --nightly moor-machine-agent';

/** Where adding a machine stands: the dialog shows one of these. */
type Step =
	| { at: 'minting' }
	| { at: 'waiting'; token: string; until: number; before: Set<string> }
	| { at: 'linked'; name: string }
	| { at: 'expired' }
	| { at: 'failed'; why: string };

/** The one line that installs the agent and enrolls this machine with `token`. */
export function enrollLine(server: string, token: string): string {
	return `${INSTALL} && ~/.local/bin/moor-machine-agent up --server ${server} --token ${token}`;
}

/** Personal machines that are online, by name. */
function onlineMine(nodes: Node[]): Set<string> {
	return new Set(nodes.flatMap((n) => (n.scope === 'personal' && n.online ? [n.name] : [])));
}

function left(until: number, now: number): string {
	const secs = Math.max(0, Math.round((until - now) / 1000));
	return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
}

/** The Add a machine button and its dialog: a fresh command, then a wait until the machine checks in. */
export function AddMachine({ csrf, server, nodes, reload }: { csrf: string; server: string; nodes: Node[]; reload: () => void }) {
	const dialog = useRef<HTMLDialogElement>(null);
	const [step, setStep] = useState<Step | null>(null);
	const [now, setNow] = useState(Date.now());

	const mint = async () => {
		setStep({ at: 'minting' });
		try {
			const t = await machineToken(csrf);
			setStep({ at: 'waiting', token: t.token, until: Date.now() + t.expires_in * 1000, before: onlineMine(nodes) });
		} catch (err) {
			setStep({ at: 'failed', why: err instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : (err as Error).message });
		}
	};

	const open = () => {
		dialog.current?.showModal();
		void mint();
	};

	const waiting = step?.at === 'waiting';
	useEffect(() => {
		if (!waiting) return;
		const tick = setInterval(() => setNow(Date.now()), 1000);
		const poll = setInterval(reload, 3000);
		return () => {
			clearInterval(tick);
			clearInterval(poll);
		};
	}, [waiting]);

	useEffect(() => {
		if (step?.at !== 'waiting') return;
		const fresh = [...onlineMine(nodes)].find((name) => !step.before.has(name));
		if (fresh) setStep({ at: 'linked', name: fresh });
		else if (now >= step.until) setStep({ at: 'expired' });
	}, [nodes, now, step]);

	const close = () => {
		dialog.current?.close();
		setStep(null);
	};

	return (
		<>
			<button class="btn primary" onClick={open}>
				Add a machine
			</button>
			<dialog ref={dialog} class="add-machine" aria-labelledby="add-machine-h" onClose={() => setStep(null)}>
				<h2 id="add-machine-h">Add a machine</h2>
				<p class="muted">
					Run your Boxes on a computer of your own. It needs Docker that your user can run without sudo. Only your Boxes run there.
				</p>
				{step?.at === 'minting' && <p aria-busy="true">Making a command for you…</p>}
				{step?.at === 'waiting' && (
					<>
						<p>Run this on the machine. It installs the agent and adds the machine to your account.</p>
						<ul class="cards">
							<Command label="Install and add" line={enrollLine(server, step.token)} />
						</ul>
						<p class="wait" role="status" aria-live="polite">
							<span class="spinner" aria-hidden="true" />
							Waiting for the machine to check in. The command works once, for {left(step.until, now)}.
						</p>
						<details class="muted">
							<summary>Agent already installed?</summary>
							<pre>
								<code>{`moor-machine-agent up --server ${server} --token ${step.token}`}</code>
							</pre>
							<p>On Linux, run loginctl enable-linger once so the agent keeps running after you log out.</p>
						</details>
					</>
				)}
				{step?.at === 'linked' && (
					<p class="notice ok" role="status">
						<strong>{step.name}</strong> is online. New Boxes of yours run there first.
					</p>
				)}
				{step?.at === 'expired' && <p class="notice warn">The command expired before a machine checked in.</p>}
				{step?.at === 'failed' && (
					<p class="notice danger" role="alert">
						{step.why}
					</p>
				)}
				<div class="actions">
					{(step?.at === 'expired' || step?.at === 'failed') && <button onClick={() => void mint()}>Make a new command</button>}
					<button class="quiet" onClick={close}>
						{step?.at === 'linked' ? 'Done' : 'Close'}
					</button>
				</div>
			</dialog>
		</>
	);
}
