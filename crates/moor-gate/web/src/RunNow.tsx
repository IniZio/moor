import { useState } from 'preact/hooks';

import { failure, toError, workflowRunNow } from './api';

/** The button that fires a workflow once, in one Box with all its repos, for pull request `pull` at its current head when given, with the answer beside it. */
export function RunNowButton({ csrf, name, onRan, hint, pull, label = 'Run now' }: { csrf: string; name: string; onRan: () => void; hint?: string; pull?: string; label?: string }) {
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
	const run = async () => {
		setBusy(true);
		setNote(null);
		try {
			const started = await workflowRunNow(csrf, name, pull);
			setNote({ ok: true, text: started.box ? `Started in ${started.box}.` : 'Started.' });
			onRan();
		} catch (err) {
			setNote({ ok: false, text: failure(toError(err)) });
		}
		setBusy(false);
	};
	return (
		<>
			<button type="button" disabled={busy} aria-label={label === 'Run now' ? `Run ${name} now${hint ?? ''}` : `${label} ${name}${hint ?? ''}`} onClick={() => void run()}>
				{label}
			</button>
			{note?.text && (
				<span class={note.ok ? 'notice ok' : 'notice danger'} role={note.ok ? 'status' : 'alert'}>
					{note.text}
				</span>
			)}
		</>
	);
}
