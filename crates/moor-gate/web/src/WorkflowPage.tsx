import { workflowGet, type Workflow } from './api';
import { useLoad } from './useLoad';
import { duration, PullLink, StatusIcon, when } from './WorkflowBits';
import { WorkflowActions } from './WorkflowActions';
import { WorkflowEditor } from './WorkflowEditor';

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
						<li key={r.id} class={r.pull ? 'has-pr' : undefined}>
							<a href={`#/workflows/${encodeURIComponent(flow.name)}/runs/${r.id}`}>
								<StatusIcon status={r.status} verdict={r.verdict} />
								<span class="grow">{r.trigger}</span>
								<span class="muted">{when(r.due_at)}</span>
								<span class="muted">{duration(r.duration_ms)}</span>
								<span class="outcome">{r.label}</span>
							</a>
							{r.pull && <PullLink pull={r.pull} />}
						</li>
					))}
				</ul>
			)}
		</>
	);
}

export function WorkflowPage({ csrf, name }: { csrf: string; name: string }) {
	const { value, reload } = useLoad(() => workflowGet(name), `runs:${name}`);
	return (
		<>
			<p><a href="#/workflows">All workflows</a></p>
			{value && <WorkflowActions csrf={csrf} flow={value} onChanged={() => void reload()} />}
			<ul class="cards">
				<li><WorkflowEditor key={`${name}:${value?.enabled}`} csrf={csrf} name={name} /></li>
			</ul>
			<Runs flow={value} />
		</>
	);
}
