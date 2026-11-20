import { useEffect, useRef, useState } from 'preact/hooks';

import {
	type BoxDetail,
	Conflict,
	repoList,
	schedulePreview,
	failure,
	type Workflow,
	workflowCreate,
	workflowEdit,
	workflowGet,
	workflowRemove,
	workflowRunNow,
	workflowSwitch,
	toError,
} from './api';
import { textProblems } from './plan';
import { GeneratePlan } from './PlanDraft';
import { PlanEditor } from './PlanEditor';
import { RepoPicker } from './RepoPicker';
import { browserZone, DAYS, EVERY, FIRST, fromCron, type Schedule, toCron, zones } from './schedule';

/** What the form opens with: a duplicate, a one-off made recurring, or nothing. */
type Draft = { name: string; repos: string[]; prompt: string; schedule: Schedule; timezone: string; timeout: string; copyOf?: string };

const DRAFT = 'moor.workflowDraft';
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;

/** A valid workflow name made from any text. */
function nameFrom(text: string): string {
	const slug = text.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[^a-z0-9]+/, '').slice(0, 64).replace(/-+$/, '');
	return slug || 'workflow';
}

function openForm(draft: Partial<Draft>) {
	sessionStorage.setItem(DRAFT, JSON.stringify(draft));
	location.hash = '#/new-workflow';
}

function takeDraft(): Partial<Draft> {
	const raw = sessionStorage.getItem(DRAFT);
	sessionStorage.removeItem(DRAFT);
	try {
		return raw ? (JSON.parse(raw) as Partial<Draft>) : {};
	} catch {
		return {};
	}
}

function fromFlow(flow: Workflow): Draft {
	return {
		name: flow.name,
		repos: flow.repos,
		prompt: flow.prompt,
		schedule: fromCron(flow.cron),
		timezone: flow.timezone ?? browserZone(),
		timeout: String(flow.timeout_minutes),
	};
}

function blank(draft: Partial<Draft>): Draft {
	return { name: '', repos: [], prompt: '', schedule: FIRST, timezone: browserZone(), timeout: '', ...draft };
}

function SchedulePicker({ value, onChange }: { value: Schedule; onChange: (s: Schedule) => void }) {
	const at = 'at' in value ? value.at : '09:00';
	const pick = (kind: string) => {
		if (kind === 'none') onChange({ kind });
		else if (kind === 'weekdays' || kind === 'daily') onChange({ kind, at });
		else if (kind === 'weekly') onChange({ kind, day: 1, at });
		else if (kind === 'hours') onChange({ kind, every: 6 });
		else onChange({ kind: 'cron', cron: toCron(value) });
	};
	return (
		<div class="wf-when">
			<select aria-label="How often" value={value.kind} onChange={(e) => pick(e.currentTarget.value)}>
				<option value="none">No schedule (run by hand)</option>
				<option value="weekdays">Every weekday</option>
				<option value="daily">Every day</option>
				<option value="weekly">Every week on</option>
				<option value="hours">Every few hours</option>
				<option value="cron">Advanced (cron)</option>
			</select>
			{value.kind === 'weekly' && (
				<select aria-label="Day" value={value.day} onChange={(e) => onChange({ ...value, day: Number(e.currentTarget.value) })}>
					{DAYS.map((d, i) => (
						<option value={i} key={d}>
							{d}
						</option>
					))}
				</select>
			)}
			{'at' in value && (
				<label>
					at <input type="time" aria-label="Time" required value={value.at} onInput={(e) => onChange({ ...value, at: e.currentTarget.value })} />
				</label>
			)}
			{value.kind === 'hours' && (
				<select aria-label="Hours apart" value={value.every} onChange={(e) => onChange({ kind: 'hours', every: Number(e.currentTarget.value) })}>
					{EVERY.map((n) => (
						<option value={n} key={n}>
							{n === 1 ? 'every hour' : `every ${n} hours`}
						</option>
					))}
				</select>
			)}
			{value.kind === 'cron' && (
				<input
					class="t-mono"
					aria-label="Cron"
					placeholder="minute hour day-of-month month day-of-week"
					required
					value={value.cron}
					onInput={(e) => onChange({ kind: 'cron', cron: e.currentTarget.value })}
				/>
			)}
		</div>
	);
}

function Preview({ cron, zone }: { cron: string; zone: string }) {
	const [shown, setShown] = useState<{ words?: string | null; next: number[]; error?: string }>({ next: [] });
	useEffect(() => {
		let live = true;
		const timer = setTimeout(() => {
			schedulePreview(cron, zone).then(
				(p) => live && setShown(p),
				(e) => live && setShown({ next: [], error: failure(e) }),
			);
		}, 250);
		return () => {
			live = false;
			clearTimeout(timer);
		};
	}, [cron, zone]);
	if (shown.error) return <p class="notice danger" role="alert">{shown.error}</p>;
	let format: Intl.DateTimeFormat;
	try {
		format = new Intl.DateTimeFormat(undefined, { timeZone: zone, weekday: 'short', day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit', hourCycle: 'h23' });
	} catch {
		return null;
	}
	return (
		<div class="wf-next" aria-live="polite">
			<span class="muted">{shown.words ? `${shown.words}. ` : ''}Next runs in {zone}:</span>
			<ol>
				{shown.next.map((ms) => (
					<li key={ms}>{format.format(ms)}</li>
				))}
			</ol>
		</div>
	);
}

/** The workflow form: a new one, a copy, a one-off made recurring, or an edit of `editing`. */
export function WorkflowForm({ csrf, editing }: { csrf: string; editing?: string }) {
	const [opened] = useState(takeDraft);
	const [flow, setFlow] = useState<Workflow | null>(null);
	const [draft, setDraft] = useState<Draft>(() => blank(opened));
	const [known, setKnown] = useState<string[]>([]);
	const [busy, setBusy] = useState(false);
	const [plan, setPlan] = useState('');
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
	const ready = nameOk && draft.repos.length > 0 && draft.prompt.trim() !== '' && (!scheduled || cron !== '') && !busy && textProblems(plan).length === 0;
	const submit = async (e: Event) => {
		e.preventDefault();
		if (!ready) return;
		setBusy(true);
		setProblem(null);
		const fields = { repos: draft.repos, prompt: draft.prompt, cron, timezone: scheduled ? draft.timezone.trim() : '', timeout_minutes: timeout };
		try {
			const saved = flow ? await workflowEdit(csrf, flow.name, fields, flow.revision) : await workflowCreate(csrf, draft.name, fields, opened.copyOf, plan.trim() || undefined);
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
					<RepoPicker repos={draft.repos} known={known} onChange={(repos) => set({ repos })} />
				</div>
				<span class="muted">Each run is one task with all of them. The agent starts in the first.</span>
			</div>
			<label>
				Prompt
				<textarea rows={8} value={draft.prompt} onInput={(e) => set({ prompt: e.currentTarget.value })} />
			</label>
			{flow?.has_plan && draft.prompt !== flow.prompt && (
				<p class="notice warn" role="status">
					This workflow has a plan made for the old prompt. Saving keeps the plan and marks it stale, so review it on the workflow page.
				</p>
			)}
			{opened.copyOf && <p class="muted">The copy keeps the plan of {opened.copyOf}, if it has one, unless you write one here.</p>}
			{!flow && (
				<div class="wf-field">
					<PlanEditor text={plan} onText={setPlan} prompt={draft.prompt} />
					<GeneratePlan csrf={csrf} repos={draft.repos} prompt={draft.prompt} dirty={plan.trim() !== ''} onPlan={setPlan} />
				</div>
			)}
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

/** Edit, Duplicate, switch on or off, and Delete, for one workflow. */
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
			<a class="button quiet" href={`#/workflows/${encodeURIComponent(flow.name)}/edit`}>
				Edit
			</a>
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

const FINISHED = ['done', 'asleep', 'closed'];

/** "Make this recurring" on a finished one-off: the form, with its repos and first prompt. */
export function MakeRecurring({ detail }: { detail: BoxDetail }) {
	if (detail.from_workflow || !FINISHED.includes(detail.state) || !detail.repo) return null;
	const repos = [detail.repo, ...detail.other_repos];
	return (
		<button
			type="button"
			class="t-btn"
			onClick={() => openForm({ name: nameFrom(detail.title), repos, prompt: detail.prompt ?? '' })}
		>
			Make this recurring
		</button>
	);
}
