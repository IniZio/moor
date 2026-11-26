import { type RunRow, type RunStatus, workflowGet, type Workflow } from './api';
import { groupByDay, groupByPull, rowTime, shortSha } from './runGroups';
import { useLoad } from './useLoad';
import { Failed } from './WorkflowBits';
import { WorkflowActions } from './WorkflowActions';
import { WorkflowEditor } from './WorkflowEditor';
import './automation.css';

/** Each run state as the row marks it: its glyph, the look its mark and words take, and the word a reader hears. */
const MARK: Record<RunStatus, { glyph: string; look: string; word: string }> = {
	queued: { glyph: '○', look: 'settled', word: 'queued' },
	running: { glyph: '◐', look: 'working', word: 'running' },
	waiting: { glyph: '●', look: 'needs', word: 'waiting on you' },
	success: { glyph: '✓', look: 'done', word: 'delivered' },
	failure: { glyph: '✗', look: 'failed', word: 'failed' },
	stopped: { glyph: '■', look: 'settled', word: 'declined' },
	skipped: { glyph: '–', look: 'settled', word: 'skipped' },
};

/** A delivered run whose outcome reads bad is marked as a warning, never as a failure. */
function markOf(r: RunRow) {
	const m = MARK[r.status];
	if (r.status !== 'success' || !r.verdict) return m;
	const word = `${m.word}: ${r.verdict.name}`;
	return r.verdict.tone === 'bad' ? { glyph: '!', look: 'warn', word } : { ...m, word };
}

function RunItem({ r }: { r: RunRow }) {
	const m = markOf(r);
	return (
		<li>
			<a href={`#/runs/${r.id}`}>
				<span class={`smk smk-${m.look}`} role="img" aria-label={m.word} title={m.word}>
					{m.glyph}
				</span>
				{r.pull && r.head_sha && <span class="t-mono">{shortSha(r.head_sha)}</span>}
				<span class={`outcome n-${m.look}`}>{r.label}</span>
				<span class="rtime">{rowTime(r.due_at)}</span>
			</a>
		</li>
	);
}

/** A pull request group's head, `OWNER/NAME #NUMBER`, linking to it on GitHub. */
function PullHead({ pull }: { pull: string }) {
	const m = /^([\w.-]+\/[\w.-]+)#(\d+)$/.exec(pull);
	if (!m) return <>{pull}</>;
	return (
		<a href={`https://github.com/${m[1]}/pull/${m[2]}`} target="_blank" rel="noopener noreferrer">
			{m[1]} #{m[2]}
		</a>
	);
}

/** The runs, by pull request when any run is for one, else by the day each was due; newest first. */
function Runs({ flow }: { flow: Workflow }) {
	const byPull = flow.runs.some((r) => r.pull);
	const groups = byPull
		? groupByPull(flow.runs).map((g) => ({ key: g.pull ?? '', head: g.pull ? <PullHead pull={g.pull} /> : 'Not for a pull request', runs: g.runs }))
		: groupByDay(flow.runs).map((g) => ({ key: g.day, head: g.day, runs: g.runs }));
	return (
		<>
			<h2 class="sub">Runs</h2>
			{groups.length === 0 && <p class="muted">No runs yet.</p>}
			{groups.map((g) => (
				<div key={g.key} class="run-group">
					<p class="dayhead">{g.head}</p>
					<ul class="runs">
						{g.runs.map((r) => (
							<RunItem key={r.id} r={r} />
						))}
					</ul>
				</div>
			))}
		</>
	);
}

export function WorkflowPage({ csrf, name }: { csrf: string; name: string }) {
	const { value: flow, setValue, error, reload } = useLoad(() => workflowGet(name), `runs:${name}`);
	if (error) return <Failed error={error} retry={reload} />;
	if (!flow) return <p class="muted" aria-busy="true">Loading {name}…</p>;
	return (
		<div class="auto">
			<div class="pagehead">
				<h1>{flow.name}</h1>
				<WorkflowActions csrf={csrf} flow={flow} onChanged={() => void reload()} />
			</div>
			<WorkflowEditor key={name} csrf={csrf} flow={flow} onFlow={setValue}>
				<Runs flow={flow} />
			</WorkflowEditor>
		</div>
	);
}
