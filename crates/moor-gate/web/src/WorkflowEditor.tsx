import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { Conflict, failure, toError, type Workflow, workflowGet, workflowSet } from './api';
import { AutomationBox } from './AutomationBox';
import { GatesPanel } from './Gates';
import { GatesNote, PlanView } from './PlanView';

/** The automation's prompt beside its file, then `children`, then moor's reading of it when there is one. */
export function WorkflowEditor({ csrf, flow, onFlow, children }: { csrf: string; flow: Workflow; onFlow: (read: Workflow) => void; children?: ComponentChildren }) {
	const [prompt, setPrompt] = useState(flow.prompt);
	const [read, setRead] = useState<string | null>(null);
	const [showFile, setShowFile] = useState(true);
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const [saved, setSaved] = useState(false);

	const adopt = (next: Workflow) => {
		onFlow(next);
		setPrompt(next.prompt);
		setRead(null);
	};
	const save = async (regenerate: boolean) => {
		setBusy(true);
		setProblem(null);
		setSaved(false);
		try {
			adopt(await workflowSet(csrf, flow.name, { prompt: regenerate ? flow.prompt : prompt, revision: flow.revision, regenerate }));
			setSaved(true);
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: err instanceof Conflict });
		}
		setBusy(false);
	};
	const reload = async () => {
		setProblem(null);
		try {
			adopt(await workflowGet(flow.name));
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: false });
		}
	};
	const gated = flow.gates != null && flow.gates.length > 0 && flow.plan == null;
	const planless = !gated && flow.plan == null && flow.plan_making == null;
	return (
		<>
			<AutomationBox
				flow={flow}
				prompt={prompt}
				read={read}
				showFile={showFile}
				busy={busy}
				problem={problem}
				saved={saved}
				onInput={(text) => {
					setPrompt(text);
					setSaved(false);
				}}
				onRead={() => setRead(prompt)}
				onSave={() => void save(false)}
				onDiscard={() => {
					setPrompt(flow.prompt);
					setRead(null);
				}}
				onReload={() => void reload()}
				onToggle={() => setShowFile(!showFile)}
			/>
			{children}
			{planless ? null : gated ? (
				<section class="plan-view" aria-label="Gates">
					<GatesNote flow={flow} busy={busy || prompt !== flow.prompt} onAgain={() => void save(true)} />
					<GatesPanel gates={flow.gates ?? []} outcomes={flow.outcomes} />
				</section>
			) : (
				<PlanView flow={flow} busy={busy || prompt !== flow.prompt} onAgain={() => void save(true)} />
			)}
		</>
	);
}
