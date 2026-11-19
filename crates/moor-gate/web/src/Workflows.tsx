import { useEffect, useState } from 'preact/hooks';

import {
	ClearsPlan,
	Conflict,
	type Line,
	type Run,
	type RunStatus,
	runAnswer,
	runGet,
	SignedOut,
	type Step,
	type Workflow,
	workflowGet,
	workflowList,
	type WorkflowRow,
	workflowRunNow,
	workflowSet,
} from './api';

const ICONS: Record<RunStatus, { glyph: string; word: string }> = {
	queued: { glyph: '○', word: 'queued' },
	running: { glyph: '●', word: 'running' },
	waiting: { glyph: '◷', word: 'waiting on you' },
	success: { glyph: '✓', word: 'delivered' },
	failure: { glyph: '✗', word: 'failed' },
	stopped: { glyph: '■', word: 'declined' },
	skipped: { glyph: '–', word: 'skipped' },
};

function reason(err: Error): string {
	return err instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : err.message;
}

function when(ms: number | null | undefined): string {
	return ms == null ? 'none' : new Date(ms).toLocaleString();
}

function duration(ms: number | null | undefined): string {
	if (ms == null) return '';
	const secs = Math.round(ms / 1000);
	if (secs < 60) return `${secs}s`;
	const mins = Math.floor(secs / 60);
	return mins < 60 ? `${mins}m ${secs % 60}s` : `${Math.floor(mins / 60)}h ${mins % 60}m`;
}

function StatusIcon({ status }: { status: RunStatus }) {
	const icon = ICONS[status];
	return <span class={`status ${status}`} role="img" aria-label={icon.word} title={icon.word}>{icon.glyph}</span>;
}

function useLoad<T>(load: () => Promise<T>, key: string) {
	const [value, setValue] = useState<T | null>(null);
	const [error, setError] = useState('');
	const reload = async () => {
		setError('');
		try {
			setValue(await load());
		} catch (err) {
			setError(err instanceof Error ? reason(err) : 'Something went wrong.');
		}
	};
	useEffect(() => {
		setValue(null);
		void reload();
	}, [key]);
	return { value, setValue, error, reload };
}

function Failed({ error, retry }: { error: string; retry: () => void }) {
	return (
		<>
			<p class="notice danger" role="alert">{error}</p>
			<button onClick={retry}>Try again</button>
		</>
	);
}

/** The button that fires a workflow once, in one Box with all its repos, with the answer beside it. */
function RunNowButton({ csrf, name, onRan, hint }: { csrf: string; name: string; onRan: () => void; hint?: string }) {
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
	const run = async () => {
		setBusy(true);
		setNote(null);
		try {
			const started = await workflowRunNow(csrf, name);
			setNote({ ok: true, text: started.box ? `Started in ${started.box}.` : 'Started.' });
			onRan();
		} catch (err) {
			setNote({ ok: false, text: err instanceof Error ? reason(err) : 'Something went wrong.' });
		}
		setBusy(false);
	};
	return (
		<>
			<button type="button" disabled={busy} aria-label={`Run ${name} now${hint ?? ''}`} onClick={() => void run()}>
				Run now
			</button>
			{note && (
				<span class={note.ok ? 'notice ok' : 'notice danger'} role={note.ok ? 'status' : 'alert'}>
					{note.text}
				</span>
			)}
		</>
	);
}

/** A workflow's repos in order: the agent starts in the first, and the rest are cloned with it. */
function RepoSet({ repos }: { repos: string[] }) {
	return (
		<span class="repo-set">
			{repos.map((r, i) => (
				<span key={r} class={i === 0 ? 'pill ok' : 'pill'} title={i === 0 ? 'The agent starts here' : undefined}>
					{r}
				</span>
			))}
		</span>
	);
}

function Row({ w, csrf, onRan }: { w: WorkflowRow; csrf: string; onRan: () => void }) {
	return (
		<li>
			<div class="row-head">
				<a href={`#/workflows/${encodeURIComponent(w.name)}`}><strong>{w.name}</strong></a>
				<span class="pills">
					{w.plan_stale && <span class="pill warn">plan stale</span>}
					<span class={w.enabled ? 'pill ok' : 'pill'}>{w.enabled ? 'enabled' : 'disabled'}</span>
				</span>
			</div>
			<dl class="facts">
				<dt>Repos</dt>
				<dd>
					<RepoSet repos={w.repos} />
				</dd>
				<dt>Cron</dt>
				<dd><code>{w.cron}</code> {w.timezone}</dd>
				<dt>Last run</dt>
				<dd>
					{w.last ? (
						<>
							<StatusIcon status={w.last.status} /> {w.last.label}
							{w.last.status === 'failure' && (
								<>
									{' '}
									<RunNowButton csrf={csrf} name={w.name} onRan={onRan} hint=" after its failed last run" />
								</>
							)}
						</>
					) : (
						'none yet'
					)}
				</dd>
				<dt>Next due</dt>
				<dd>{w.enabled ? when(w.next_due) : (w.disabled_reason ?? 'disabled')}</dd>
			</dl>
			{w.last?.status !== 'failure' && (
				<p class="run-now">
					<RunNowButton csrf={csrf} name={w.name} onRan={onRan} />
				</p>
			)}
		</li>
	);
}

function List({ csrf }: { csrf: string }) {
	const { value, error, reload } = useLoad(workflowList, 'list');
	if (error) return <Failed error={error} retry={reload} />;
	if (!value) return <p class="muted" aria-busy="true">Loading workflows…</p>;
	return value.length === 0 ? (
		<p class="muted">No workflows yet. Add one with the <code>add_workflow</code> tool in Claude Code.</p>
	) : (
		<ul class="cards">{value.map((w) => <Row key={w.name} w={w} csrf={csrf} onRan={() => void reload()} />)}</ul>
	);
}

function WorkflowEditor({ csrf, name }: { csrf: string; name: string }) {
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
			setProblem({ text: err instanceof Error ? reason(err) : 'Something went wrong.', stale: false });
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

	const save = async (clearPlan: boolean) => {
		setBusy(true);
		setProblem(null);
		setConfirm('');
		setSaved(false);
		try {
			adopt(await workflowSet(csrf, name, { prompt, plan, revision: flow.revision, clearPlan }));
			setSaved(true);
		} catch (err) {
			if (err instanceof ClearsPlan) setConfirm(err.message);
			else setProblem({ text: err instanceof Error ? reason(err) : 'Something went wrong.', stale: err instanceof Conflict });
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
	const dirty = prompt !== flow.prompt || plan !== (flow.plan ?? '');
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
				<RepoSet repos={flow.repos} /> · <code>{flow.cron}</code> {flow.timezone} · {flow.enabled ? 'enabled' : 'disabled'}
			</p>
			{flow.plan_stale && (
				<p class="notice warn" role="status">
					The plan was made from a different prompt. It still runs. Review it against the prompt.
				</p>
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
				<label>
					Plan (JSON)
					<textarea
						aria-label="Plan"
						rows={16}
						spellcheck={false}
						placeholder="No plan: each run delivers the prompt in one step."
						value={plan}
						onInput={(e) => setPlan(e.currentTarget.value)}
					/>
				</label>
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
			{problem && <pre class="notice danger" role="alert">{problem.text}</pre>}
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

function Runs({ flow }: { flow: Workflow | null }) {
	if (!flow) return null;
	return (
		<>
			<h2>Runs</h2>
			{flow.runs.length === 0 ? (
				<p class="muted">No runs yet.</p>
			) : (
				<ul class="runs">
					{flow.runs.map((r) => (
						<li key={r.id}>
							<a href={`#/workflows/${encodeURIComponent(flow.name)}/runs/${r.id}`}>
								<StatusIcon status={r.status} />
								<span class="grow">{r.trigger}</span>
								<span class="muted">{when(r.due_at)}</span>
								<span class="muted">{duration(r.duration_ms)}</span>
								<span class="outcome">{r.label}</span>
							</a>
						</li>
					))}
				</ul>
			)}
		</>
	);
}

function WorkflowPage({ csrf, name }: { csrf: string; name: string }) {
	const { value } = useLoad(() => workflowGet(name), `runs:${name}`);
	return (
		<>
			<p><a href="#/workflows">All workflows</a></p>
			<ul class="cards">
				<li><WorkflowEditor key={name} csrf={csrf} name={name} /></li>
			</ul>
			<Runs flow={value} />
		</>
	);
}

function LineItem({ line }: { line: Line }) {
	return (
		<li>
			<span class="muted">{when(line.at)}</span> {line.text}
			{line.evidence.length > 0 && (
				<ul class="evidence">
					{line.evidence.map((e) => (
						<li key={e.name}>
							{e.href ? <a href={e.href} download>{e.name}</a> : e.name}
						</li>
					))}
				</ul>
			)}
		</li>
	);
}

const STATE_WORD: Record<Step['state'], string> = {
	pending: 'not reached',
	running: 'running',
	waiting: 'waiting on you',
	passed: 'done',
	failed: 'failed',
	ended: 'ended',
};

function StepItem({ step, number }: { step: Step; number: number }) {
	return (
		<li>
			<details open={step.state === 'waiting' || step.state === 'failed'}>
				<summary>
					<span class={`kind ${step.kind}`}>{step.kind}</span>
					<span class="grow">{number}. {step.title}</span>
					<span class={`state ${step.state}`}>{STATE_WORD[step.state]}</span>
				</summary>
				{step.instructions && <p class="muted">{step.instructions}</p>}
				{step.command && <p class="muted">Runs <code>{step.command}</code></p>}
				{step.question && <p class="muted">Asks: {step.question}</p>}
				{step.done_when && <p class="muted">Done when: {step.done_when}</p>}
				{step.history.length > 0 ? (
					<ul class="history">{step.history.map((l) => <LineItem key={`${l.at}${l.kind}`} line={l} />)}</ul>
				) : (
					<p class="muted">This step was not reached.</p>
				)}
				{step.check && (
					<>
						<p class="muted">Check output, last 4 KiB, exit {step.check.exit ?? 'unknown'}</p>
						<pre aria-label={`Output of ${step.title}`}>{step.check.tail || '(no output)'}</pre>
					</>
				)}
			</details>
		</li>
	);
}

function Question({ run, csrf, onAnswered }: { run: Run; csrf: string; onAnswered: (run: Run) => void }) {
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState('');
	if (!run.question) return null;
	const answer = async (yes: boolean) => {
		setBusy(true);
		setProblem('');
		try {
			onAnswered(await runAnswer(csrf, run.workflow, run.id, yes));
		} catch (err) {
			setProblem(err instanceof Error ? reason(err) : 'Something went wrong.');
		}
		setBusy(false);
	};
	return (
		<div class="notice warn" role="group" aria-label="Question">
			<p><strong>{run.question.text || 'This run waits for your answer.'}</strong></p>
			<div class="actions">
				<button disabled={busy} onClick={() => void answer(true)}>Yes</button>
				<button disabled={busy} onClick={() => void answer(false)}>No</button>
			</div>
			{problem && <p class="notice danger" role="alert">{problem}</p>}
		</div>
	);
}

function RunPage({ csrf, name, id }: { csrf: string; name: string; id: number }) {
	const { value: run, setValue, error, reload } = useLoad(() => runGet(name, id), `${name}/${id}`);
	return (
		<>
			<p><a href={`#/workflows/${encodeURIComponent(name)}`}>{name}</a></p>
			{error && <Failed error={error} retry={reload} />}
			{!run && !error && <p class="muted" aria-busy="true">Loading run…</p>}
			{run && (
				<>
					<div class="row-head">
						<h2 class="run-title">
							<StatusIcon status={run.status} /> Run {run.id}
						</h2>
						<span class="pill">{run.label}</span>
						{run.status === 'failure' && (
							<RunNowButton csrf={csrf} name={run.workflow} onRan={reload} hint=" after this failed run" />
						)}
					</div>
					<p class="muted">
						Due {when(run.due_at)}
						{run.duration_ms != null && `, took ${duration(run.duration_ms)}`}
						{run.box && `, Box ${run.box}`}
					</p>
					{run.result && <p class="notice ok">{run.result}</p>}
					{run.detail && <p class="notice danger">{run.detail}</p>}
					<Question run={run} csrf={csrf} onAnswered={setValue} />
					<ol class="steps">{run.steps.map((s, i) => <StepItem key={s.id} step={s} number={i + 1} />)}</ol>
					{run.lines.length > 0 && <ul class="history">{run.lines.map((l) => <LineItem key={`${l.at}${l.kind}`} line={l} />)}</ul>}
					<div class="actions"><button class="quiet" onClick={reload}>Refresh</button></div>
				</>
			)}
		</>
	);
}

/** What the address names under `#/workflows`: nothing, a workflow, or one of its runs. */
export type At = { page: 'list' } | { page: 'workflow'; name: string } | { page: 'run'; name: string; id: number };

export function workflowsAt(hash: string): At | null {
	const rest = hash.replace(/^#\/?/, '');
	if (rest === 'workflows') return { page: 'list' };
	if (!rest.startsWith('workflows/')) return null;
	const parts = rest.slice('workflows/'.length).split('/').map(decodeURIComponent);
	const [name, runs, id] = parts;
	if (name && runs === 'runs' && /^\d+$/.test(id ?? '')) return { page: 'run', name, id: Number(id) };
	return name ? { page: 'workflow', name } : { page: 'list' };
}

export function Workflows({ csrf, at }: { csrf: string; at: At }) {
	return (
		<section>
			{at.page === 'list' && (
				<>
					<h2>Workflows</h2>
					<List csrf={csrf} />
				</>
			)}
			{at.page === 'workflow' && <WorkflowPage csrf={csrf} name={at.name} />}
			{at.page === 'run' && <RunPage csrf={csrf} name={at.name} id={at.id} />}
		</section>
	);
}
