import { useRef, useState } from 'preact/hooks';

import { failure, toError, type Workflow, workflowRemove, workflowRunNow, workflowSwitch } from './api';
import { fromFlow, openForm } from './WorkflowDraft';

/** Run now, Duplicate, switch on or off, and Delete, for one workflow. */
export function WorkflowActions({ csrf, flow, onChanged }: { csrf: string; flow: Workflow; onChanged: () => void }) {
	const confirm = useRef<HTMLDialogElement>(null);
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState('');
	const act = async (go: () => Promise<void>) => {
		setBusy(true);
		setProblem('');
		try {
			await go();
		} catch (e) {
			setProblem(failure(toError(e)));
		}
		setBusy(false);
	};
	const duplicate = () =>
		openForm({ ...fromFlow(flow), name: `${flow.name}-copy`.slice(0, 64), copyOf: flow.name });
	return (
		<div class="actions wf-actions">
			<button
				type="button"
				disabled={busy}
				onClick={() =>
					void act(async () => {
						const started = await workflowRunNow(csrf, flow.name);
						if (started.box) location.hash = `#/box/${encodeURIComponent(started.box)}`;
					})
				}
			>
				Run now
			</button>
			<button type="button" class="quiet" onClick={duplicate}>
				Duplicate
			</button>
			<button type="button" class="quiet" disabled={busy} onClick={() =>
					void act(async () => {
						await workflowSwitch(csrf, flow.name, !flow.enabled);
						onChanged();
					})
				}>
				{flow.enabled ? 'Disable' : 'Enable'}
			</button>
			<button type="button" class="quiet wf-danger" disabled={busy} onClick={() => confirm.current?.showModal()}>
				Delete
			</button>
			{problem && <span class="notice danger" role="alert">{problem}</span>}
			<dialog ref={confirm} class="wf-confirm" aria-label={`Delete ${flow.name}?`}>
				<form method="dialog">
					<p>
						<strong>Delete {flow.name}?</strong>
					</p>
					<p class="muted">It stops running. Tasks it already started stay.</p>
					<div class="actions">
						<button
							type="button"
							class="wf-danger-fill"
							onClick={() => {
								confirm.current?.close();
								void act(async () => {
									await workflowRemove(csrf, flow.name);
									location.hash = '#/workflows';
								});
							}}
						>
							Delete
						</button>
						<button class="quiet" value="cancel">
							Keep it
						</button>
					</div>
				</form>
			</dialog>
		</div>
	);
}
