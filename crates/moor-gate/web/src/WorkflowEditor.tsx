import { useEffect, useState } from 'preact/hooks';

import { ClearsPlan, Conflict, failure, toError, type Workflow, workflowGet, workflowSet } from './api';
import { GatesPanel } from './Gates';
import { GeneratePlan } from './PlanDraft';
import { PlanEditor } from './PlanEditor';
import { reviewed, sameText, textProblems } from './plan';
import { scheduleText } from './schedule';
import { useLoad } from './useLoad';
import { Failed, PullTag, RepoSet, when } from './WorkflowBits';

export function WorkflowEditor({ csrf, name }: { csrf: string; name: string }) {
	const { value: flow, setValue, error, reload } = useLoad(() => workflowGet(name), name);
	const [prompt, setPrompt] = useState('');
	const [plan, setPlan] = useState('');
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const [confirm, setConfirm] = useState('');
	const [saved, setSaved] = useState(false);

	const adopt = (read: Workflow) => {
		setValue(read);
		setPrompt(read.prompt);
		setPlan(read.plan ?? '');
	};
	const load = async () => {
		setProblem(null);
		setConfirm('');
		setSaved(false);
		try {
			adopt(await workflowGet(name));
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: false });
		}
	};
	useEffect(() => {
		if (flow && prompt === '' && plan === '') {
			setPrompt(flow.prompt);
			setPlan(flow.plan ?? '');
		}
	}, [flow]);

	if (error) return <Failed error={error} retry={reload} />;
	if (!flow) return <p class="muted" aria-busy="true">Loading {name}…</p>;

	const save = async (clearPlan: boolean, draft = { prompt, plan }) => {
		setBusy(true);
		setProblem(null);
		setConfirm('');
		setSaved(false);
		try {
			adopt(await workflowSet(csrf, name, { ...draft, revision: flow.revision, clearPlan }));
			setSaved(true);
		} catch (err) {
			if (err instanceof ClearsPlan) setConfirm(err.message);
			else setProblem({ text: failure(toError(err)), stale: err instanceof Conflict });
		}
		setBusy(false);
	};
	const submit = (e: Event) => {
		e.preventDefault();
		const clears = plan.trim() === '' && flow.plan != null;
		if (!clears) return void save(false);
		setConfirm(
			prompt !== flow.prompt
				? 'This changes the prompt and leaves the plan empty, so the plan would be cleared and every run would deliver the prompt in one step.'
				: 'The plan is empty, so it would be cleared and every run would deliver the prompt in one step.',
		);
	};
	const dirty = prompt !== flow.prompt || sameText(plan) !== sameText(flow.plan ?? '');
	const planProblems = textProblems(plan);
	const goesStale = prompt !== flow.prompt && plan.trim() !== '';
	return (
		<form onSubmit={submit}>
			<div class="row-head">
				<strong>{flow.name}</strong>
				<span class="pills">
					{flow.plan_edited && <span class="pill">edited</span>}
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
			{flow.plan_stale && flow.plan && (
				<div class="notice warn plan-stale" role="status">
					<p>The plan was made from a different prompt. It still runs. Review its steps against the prompt.</p>
					<button
						type="button"
						disabled={busy || dirty}
						title={dirty ? 'Save or reload your changes first' : undefined}
						onClick={() => void save(false, { prompt: flow.prompt, plan: reviewed(flow.plan ?? '') })}
					>
						Mark reviewed
					</button>
				</div>
			)}
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
				{flow.gates && flow.gates.length > 0 && flow.plan == null ? (
					<GatesPanel gates={flow.gates} />
				) : (
					<div>
						<PlanEditor text={plan} onText={setPlan} prompt={prompt} />
						<GeneratePlan csrf={csrf} repos={flow.repos} prompt={prompt} dirty={sameText(plan) !== sameText(flow.plan ?? '')} onPlan={setPlan} />
					</div>
				)}
			</div>
			{goesStale && (
				<p class="notice warn" role="status">Saving a changed prompt with this plan marks the plan stale.</p>
			)}
			{confirm && (
				<div class="notice warn" role="alertdialog" aria-label="Clear the plan?">
					<p>{confirm}</p>
					<div class="actions">
						<button type="button" disabled={busy} onClick={() => void save(true)}>Save and clear the plan</button>
						<button type="button" class="quiet" onClick={() => setConfirm('')}>Keep editing</button>
					</div>
				</div>
			)}
			{problem?.text && <pre class="notice danger" role="alert">{problem.text}</pre>}
			{saved && <p class="notice ok" role="status">Saved. The workflow is at revision {flow.revision}.</p>}
			<div class="actions">
				<button disabled={busy || !dirty || planProblems.length > 0}>Save</button>
				<button type="button" class="quiet" disabled={busy} onClick={load}>
					{problem?.stale ? 'Discard mine and load theirs' : 'Reload'}
				</button>
			</div>
		</form>
	);
}
