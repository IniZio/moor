import { type RunRow, workflowGet, type Workflow } from './api';
import { groupByPull, shortSha } from './runGroups';
import { scheduleText } from './schedule';
import { useLoad } from './useLoad';
import { duration, Failed, PullLink, PullTag, StatusIcon, when } from './WorkflowBits';
import { WorkflowActions } from './WorkflowActions';
import { WorkflowEditor } from './WorkflowEditor';
import './automation.css';

function RunItem({ flow, r }: { flow: Workflow; r: RunRow }) {
	return (
		<li>
			<a href={`#/workflows/${encodeURIComponent(flow.name)}/runs/${r.id}`}>
				<StatusIcon status={r.status} verdict={r.verdict} />
				<span class="grow">{r.pull && r.head_sha ? <span class="t-mono">{shortSha(r.head_sha)}</span> : r.trigger}</span>
				<span class="muted">{when(r.due_at)}</span>
				<span class="muted">{duration(r.duration_ms)}</span>
				<span class="outcome">{r.label}</span>
			</a>
		</li>
	);
}

/** The runs, grouped by the pull request each is for, newest group first. */
function Runs({ flow }: { flow: Workflow }) {
	const groups = groupByPull(flow.runs);
	const flat = groups.length === 1 && groups[0].pull == null;
	return (
		<>
			<h2 class="sub">Runs</h2>
			{groups.length === 0 && <p class="muted">No runs yet.</p>}
			{groups.map((g) => (
				<div key={g.pull ?? ''} class="run-group">
					{!flat && <p class="dayhead">{g.pull ? <PullLink pull={g.pull} /> : 'Not for a pull request'}</p>}
					<ul class="runs">
						{g.runs.map((r) => (
							<RunItem key={r.id} flow={flow} r={r} />
						))}
					</ul>
				</div>
			))}
		</>
	);
}

/** What fires it, on which repos, and who each run acts as. */
function Trigger({ flow }: { flow: Workflow }) {
	return (
		<p class="trig">
			<span title={flow.cron ?? undefined}>{scheduleText(flow.words, flow.cron, flow.timezone)}</span>
			{flow.on_pull_request && (
				<>
					{' '}
					<PullTag />
				</>
			)}
			{' · on '}
			<span class="t-mono">{flow.repos.join(', ')}</span>
			{flow.run_as && (
				<>
					{' · acts as '}
					<span class="t-mono">{flow.run_as}</span>
				</>
			)}
			{flow.cron != null && <> · {flow.enabled ? `next run ${flow.next_text ?? when(flow.next_due)}` : 'disabled'}</>}
			{flow.cron == null && !flow.enabled && ' · disabled'}
		</p>
	);
}

export function WorkflowPage({ csrf, name }: { csrf: string; name: string }) {
	const { value: flow, setValue, error, reload } = useLoad(() => workflowGet(name), `runs:${name}`);
	if (error) return <Failed error={error} retry={reload} />;
	if (!flow) return <p class="muted" aria-busy="true">Loading {name}…</p>;
	return (
		<div class="auto">
			<p class="back">
				<a href="#/workflows">All workflows</a>
			</p>
			<div class="pagehead">
				<h1>{flow.name}</h1>
				<WorkflowActions csrf={csrf} flow={flow} onChanged={() => void reload()} />
			</div>
			<Trigger flow={flow} />
			<p class="rev">
				revision {flow.revision}
				{flow.plan_stale && <span class="pill warn">plan stale</span>}
				<span class="grow" />
				<a href={`#/workflows/${encodeURIComponent(flow.name)}/edit`}>Edit schedule and repos</a>
			</p>
			<WorkflowEditor key={name} csrf={csrf} flow={flow} onFlow={setValue} />
			<Runs flow={flow} />
		</div>
	);
}
