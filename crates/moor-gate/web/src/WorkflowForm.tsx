import { useEffect, useState } from 'preact/hooks';

import { Conflict, failure, repoList, toError, type Workflow, workflowCreate, workflowEdit, workflowGet } from './api';
import { PullSwitch } from './PullSwitch';
import { RepoMenu } from './RepoMenu';
import { toCron, zones } from './schedule';
import { Preview, SchedulePicker } from './SchedulePicker';
import { blank, type Draft, fromFlow, takeDraft } from './WorkflowDraft';

const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;

/** The workflow form: a new one, a copy, a one-off made recurring, or an edit of `editing`. */
export function WorkflowForm({ csrf, editing }: { csrf: string; editing?: string }) {
	const [opened] = useState(takeDraft);
	const [flow, setFlow] = useState<Workflow | null>(null);
	const [draft, setDraft] = useState<Draft>(() => blank(opened));
	const [known, setKnown] = useState<string[]>([]);
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const load = () => {
		if (!editing) return;
		setProblem(null);
		workflowGet(editing).then(
			(read) => {
				setFlow(read);
				setDraft(fromFlow(read));
			},
			(e) => setProblem({ text: failure(e), stale: false }),
		);
	};
	useEffect(() => {
		repoList().then(setKnown, () => setKnown([]));
		load();
	}, [editing]);
	if (editing && !flow) {
		return problem?.text ? <p class="notice danger" role="alert">{problem.text}</p> : <p class="muted" aria-busy="true">Loading {editing}…</p>;
	}
	const set = (part: Partial<Draft>) => setDraft({ ...draft, ...part });
	const cron = toCron(draft.schedule);
	const timeout = draft.timeout.trim() === '' ? undefined : Number(draft.timeout);
	const nameOk = NAME.test(draft.name);
	const scheduled = draft.schedule.kind !== 'none';
	const ready = nameOk && draft.repos.length > 0 && draft.prompt.trim() !== '' && (!scheduled || cron !== '') && !busy;
	const submit = async (e: Event) => {
		e.preventDefault();
		if (!ready) return;
		setBusy(true);
		setProblem(null);
		const fields = { repos: draft.repos, prompt: draft.prompt, cron, timezone: scheduled ? draft.timezone.trim() : '', timeout_minutes: timeout, on_pull_request: draft.onPull };
		try {
			const saved = flow ? await workflowEdit(csrf, flow.name, fields, flow.revision) : await workflowCreate(csrf, draft.name, fields, opened.copyOf);
			location.hash = `#/workflows/${encodeURIComponent(saved.name)}`;
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: flow != null && err instanceof Conflict });
		}
		setBusy(false);
	};
	return (
		<form class="wf-form" onSubmit={(e) => void submit(e)}>
			<p>
				<a href={flow ? `#/workflows/${encodeURIComponent(flow.name)}` : '#/workflows'}>{flow ? flow.name : 'All workflows'}</a>
			</p>
			<h2>{flow ? `Edit ${flow.name}` : opened.copyOf ? `Duplicate ${opened.copyOf}` : 'New workflow'}</h2>
			<label>
				Name
				<input
					class="t-mono"
					value={draft.name}
					readOnly={flow != null}
					aria-invalid={draft.name !== '' && !nameOk}
					placeholder="nightly-deps"
					onInput={(e) => set({ name: e.currentTarget.value })}
				/>
				<span class="muted">{flow ? 'A workflow keeps its name.' : 'Lowercase letters, digits, dots, dashes and underscores.'}</span>
			</label>
			<div class="wf-field">
				<span>Repos</span>
				<div class="wf-repos">
					<RepoMenu repos={draft.repos} known={known} rows={[]} onChange={(repos) => set({ repos })} />
				</div>
				<span class="muted">Each run is one task with all of them. The agent starts in the first.</span>
			</div>
			<label>
				Prompt
				<textarea rows={8} value={draft.prompt} onInput={(e) => set({ prompt: e.currentTarget.value })} />
			</label>
			{flow?.has_plan && draft.prompt !== flow.prompt && (
				<p class="notice warn" role="status">
					moor made this workflow's plan from the old prompt. Saving keeps that plan, marked stale, until moor makes it again.
				</p>
			)}
			{!flow && <p class="muted">Write the prompt only. moor reads the repos and the prompt, then makes the plan.</p>}
			<div class="wf-field">
				<span>Schedule</span>
				<SchedulePicker value={draft.schedule} onChange={(schedule) => set({ schedule })} />
				{scheduled ? (
					<>
						<label class="wf-zone">
							Time zone
							<input list="wf-zones" value={draft.timezone} onInput={(e) => set({ timezone: e.currentTarget.value })} />
						</label>
						<datalist id="wf-zones">
							{zones().map((z) => (
								<option value={z} key={z} />
							))}
						</datalist>
						{draft.schedule.kind !== 'cron' && <span class="muted t-mono" title="The same schedule as cron">{cron}</span>}
						<Preview cron={cron} zone={draft.timezone.trim()} />
					</>
				) : (
					<span class="muted">It runs only when you press Run now or call run_workflow, or on a pull request if it has that trigger.</span>
				)}
			</div>
			<PullSwitch on={draft.onPull} onChange={(onPull) => set({ onPull })} />
			<details>
				<summary>More</summary>
				<label>
					Timeout in minutes
					<input type="number" min={1} max={720} placeholder="180" value={draft.timeout} onInput={(e) => set({ timeout: e.currentTarget.value })} />
					<span class="muted">A run that takes longer is stopped. Three hours when blank.</span>
				</label>
			</details>
			{problem?.text && <p class="notice danger" role="alert">{problem.text}</p>}
			<div class="actions">
				<button disabled={!ready}>{busy ? 'Saving…' : flow ? 'Save' : 'Create workflow'}</button>
				{problem?.stale && (
					<button type="button" class="quiet" onClick={load}>
						Discard mine and load theirs
					</button>
				)}
			</div>
		</form>
	);
}
