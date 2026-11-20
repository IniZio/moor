import { RunPage } from './RunPage';
import { WorkflowForm } from './WorkflowForm';
import { List } from './WorkflowList';
import { WorkflowPage } from './WorkflowPage';

/** What the address names under `#/workflows`: nothing, a workflow, or one of its runs. */
export type At =
	| { page: 'list' }
	| { page: 'new' }
	| { page: 'workflow'; name: string }
	| { page: 'edit'; name: string }
	| { page: 'run'; name: string; id: number };

export function workflowsAt(hash: string): At | null {
	const rest = hash.replace(/^#\/?/, '');
	if (rest === 'workflows') return { page: 'list' };
	if (rest === 'new-workflow') return { page: 'new' };
	if (!rest.startsWith('workflows/')) return null;
	const parts = rest.slice('workflows/'.length).split('/').map(decodeURIComponent);
	const [name, runs, id] = parts;
	if (name && runs === 'runs' && /^\d+$/.test(id ?? '')) return { page: 'run', name, id: Number(id) };
	if (name && runs === 'edit') return { page: 'edit', name };
	return name ? { page: 'workflow', name } : { page: 'list' };
}

export function Workflows({ csrf, at }: { csrf: string; at: At }) {
	return (
		<section>
			{at.page === 'list' && (
				<>
					<div class="row-head wf-head">
						<h2>Workflows</h2>
						<a class="button" href="#/new-workflow">New workflow</a>
					</div>
					<List csrf={csrf} />
				</>
			)}
			{at.page === 'new' && <WorkflowForm csrf={csrf} />}
			{at.page === 'edit' && <WorkflowForm key={at.name} csrf={csrf} editing={at.name} />}
			{at.page === 'workflow' && <WorkflowPage csrf={csrf} name={at.name} />}
			{at.page === 'run' && <RunPage csrf={csrf} name={at.name} id={at.id} />}
		</section>
	);
}
