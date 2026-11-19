import { useRef, useState } from 'preact/hooks';

import { type Node, failure } from './api';
import { machineRemove } from './settingsApi';

/** Remove, behind a confirm: the machine leaves the account; its agent keeps running until stopped there. */
export function RemoveMachine({ csrf, node, onRemoved }: { csrf: string; node: Node; onRemoved: () => void }) {
	const dialog = useRef<HTMLDialogElement>(null);
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState('');
	const remove = async () => {
		setBusy(true);
		setProblem('');
		try {
			await machineRemove(csrf, node.name);
			dialog.current?.close();
			onRemoved();
		} catch (err) {
			setProblem(failure(err));
		}
		setBusy(false);
	};
	const running = node.used > 0;
	return (
		<>
			<button type="button" class="quiet wf-danger" onClick={() => dialog.current?.showModal()}>
				Remove
			</button>
			<dialog ref={dialog} class="wf-confirm" aria-labelledby={`rm-${node.name}`}>
				<h2 id={`rm-${node.name}`}>Remove {node.name}?</h2>
				{running ? (
					<p class="notice warn">
						{node.used} {node.used === 1 ? 'Box runs' : 'Boxes run'} there now. Close {node.used === 1 ? 'it' : 'them'} or let {node.used === 1 ? 'it' : 'them'} sleep
						first.
					</p>
				) : (
					<p>New Boxes stop going there. To stop the agent on the machine too, run this on it:</p>
				)}
				{!running && (
					<pre>
						<code>moor-machine-agent down</code>
					</pre>
				)}
				{problem && (
					<p class="notice danger" role="alert">
						{problem}
					</p>
				)}
				<div class="actions">
					<button class="wf-danger-fill" disabled={busy || running} onClick={() => void remove()}>
						{busy ? 'Removing…' : 'Remove machine'}
					</button>
					<button class="quiet" onClick={() => dialog.current?.close()}>
						Cancel
					</button>
				</div>
			</dialog>
		</>
	);
}
