import { type RunRow, workflowGet, type Workflow } from './api';
import { usePoll } from './Portal';
import { groupByDay, groupByPull, rowTime, shortSha } from './runGroups';
import { Failed, markOf, StatusIcon } from './WorkflowBits';
import { WorkflowActions } from './WorkflowActions';
import { WorkflowEditor } from './WorkflowEditor';
import './automation.css';

function RunItem({ r }: { r: RunRow }) {
	const m = markOf(r.status, r.verdict);
	return (
		<li>
			<a href={`#/runs/${r.id}`}>
				<StatusIcon status={r.status} verdict={r.verdict} />
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
	// Polled as a task page is, so a reading that lands or a run that starts shows without a reload.
	const { value: flow, setValue, error, reload } = usePoll(() => workflowGet(name), `runs:${name}`);
	if (error && !flow) return <Failed error={error} retry={reload} />;
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
