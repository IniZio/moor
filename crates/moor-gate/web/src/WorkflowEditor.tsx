import { useEffect, useState } from 'preact/hooks';

import { Conflict, failure, toError, type Workflow, workflowGet, workflowSet } from './api';
import { GatesPanel } from './Gates';
import { PlanView } from './PlanView';
import { scheduleText } from './schedule';
import { useLoad } from './useLoad';
import { Failed, PullTag, RepoSet, when } from './WorkflowBits';

export function WorkflowEditor({ csrf, name }: { csrf: string; name: string }) {
	const { value: flow, setValue, error, reload } = useLoad(() => workflowGet(name), name);
	const [prompt, setPrompt] = useState('');
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const [saved, setSaved] = useState(false);

	const adopt = (read: Workflow) => {
		setValue(read);
		setPrompt(read.prompt);
	};
	const load = async () => {
		setProblem(null);
		setSaved(false);
		try {
			adopt(await workflowGet(name));
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: false });
		}
	};
	useEffect(() => {
		if (flow && prompt === '') setPrompt(flow.prompt);
	}, [flow]);

	if (error) return <Failed error={error} retry={reload} />;
	if (!flow) return <p class="muted" aria-busy="true">Loading {name}…</p>;

	const save = async (regenerate: boolean) => {
		setBusy(true);
		setProblem(null);
		setSaved(false);
		try {
			adopt(await workflowSet(csrf, name, { prompt: regenerate ? flow.prompt : prompt, revision: flow.revision, regenerate }));
			setSaved(true);
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: err instanceof Conflict });
		}
		setBusy(false);
	};
	const dirty = prompt !== flow.prompt;
	const gated = flow.gates != null && flow.gates.length > 0 && flow.plan == null;
	return (
		<form
			onSubmit={(e) => {
				e.preventDefault();
				void save(false);
			}}
		>
			<div class="row-head">
				<strong>{flow.name}</strong>
				<span class="pills">
					{flow.plan_stale && <span class="pill warn">plan stale</span>}
					<span class="pill">Revision {flow.revision}</span>
				</span>
			</div>
			<p class="muted">
				<RepoSet repos={flow.repos} /> · <span title={flow.cron ?? undefined}>{scheduleText(flow.words, flow.cron, flow.timezone)}</span>
				{flow.on_pull_request && (
					<>
						{' '}
						<PullTag />
					</>
				)}
				{flow.cron != null && <> · {flow.enabled ? `next run ${flow.next_text ?? when(flow.next_due)}` : 'disabled'}</>}
				{flow.cron == null && !flow.enabled && ' · disabled'}
			</p>
			<div class="sides">
				<label>
					Prompt
					<textarea
						aria-label="Prompt"
						rows={16}
						spellcheck={false}
						value={prompt}
						onInput={(e) => setPrompt(e.currentTarget.value)}
					/>
				</label>
				{gated ? (
					<div>
						<p class="muted">moor's current reading of this workflow is a set of gates. Regenerating them from the prompt comes later.</p>
						<GatesPanel gates={flow.gates ?? []} outcomes={flow.outcomes} />
					</div>
				) : (
					<PlanView flow={flow} busy={busy || dirty} onAgain={() => void save(true)} />
				)}
			</div>
			{problem?.text && <pre class="notice danger" role="alert">{problem.text}</pre>}
			{saved && <p class="notice ok" role="status">Saved. The workflow is at revision {flow.revision}.</p>}
			<div class="actions">
				<button disabled={busy || !dirty}>Save</button>
				<button type="button" class="quiet" disabled={busy} onClick={load}>
					{problem?.stale ? 'Discard mine and load theirs' : 'Reload'}
				</button>
			</div>
		</form>
	);
}
