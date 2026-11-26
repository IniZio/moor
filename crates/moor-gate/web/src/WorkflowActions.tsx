import { useRef, useState } from 'preact/hooks';

import { failure, toError, type Workflow, workflowRemove, workflowRunNow, workflowSwitch } from './api';
import { fromFlow, openForm } from './WorkflowDraft';

/** One "⋯" button holding Run now, Duplicate, Edit schedule and repos, switch on or off, and Delete. */
export function WorkflowActions({ csrf, flow, onChanged }: { csrf: string; flow: Workflow; onChanged: () => void }) {
	const confirm = useRef<HTMLDialogElement>(null);
	const menu = useRef<HTMLDivElement>(null);
	const opener = useRef<HTMLButtonElement>(null);
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState('');
	const act = async (go: () => Promise<void>) => {
		menu.current?.hidePopover();
		setBusy(true);
		setProblem('');
		try {
			await go();
		} catch (e) {
			setProblem(failure(toError(e)));
		}
		setBusy(false);
	};
	const place = () => {
		const at = opener.current?.getBoundingClientRect();
		if (!at || !menu.current) return;
		menu.current.style.top = `${at.bottom + 4}px`;
		menu.current.style.right = `${document.documentElement.clientWidth - at.right}px`;
	};
	const duplicate = () => {
		menu.current?.hidePopover();
		openForm({ ...fromFlow(flow), name: `${flow.name}-copy`.slice(0, 64), copyOf: flow.name });
	};
	const id = `wf-menu-${flow.name}`;
	return (
		<div class="wf-actions">
			{problem && <span class="notice danger" role="alert">{problem}</span>}
			<button ref={opener} type="button" class="wf-more" popovertarget={id} aria-label={`More for ${flow.name}`} title="More" disabled={busy}>
				⋯
			</button>
			<div ref={menu} id={id} class="pop wf-menu" popover="auto" role="menu" onBeforeToggle={place}>
				<button
					type="button"
					role="menuitem"
					onClick={() =>
						void act(async () => {
							const started = await workflowRunNow(csrf, flow.name);
							if (started.box) location.hash = `#/box/${encodeURIComponent(started.box)}`;
						})
					}
				>
					Run now
				</button>
				<button type="button" role="menuitem" onClick={duplicate}>
					Duplicate
				</button>
				<a role="menuitem" href={`#/workflows/${encodeURIComponent(flow.name)}/edit`}>
					Edit schedule and repos
				</a>
				<button
					type="button"
					role="menuitem"
					onClick={() =>
						void act(async () => {
							await workflowSwitch(csrf, flow.name, !flow.enabled);
							onChanged();
						})
					}
				>
					{flow.enabled ? 'Disable' : 'Enable'}
				</button>
				<button
					type="button"
					role="menuitem"
					class="wf-danger"
					onClick={() => {
						menu.current?.hidePopover();
						confirm.current?.showModal();
					}}
				>
					Delete
				</button>
			</div>
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
