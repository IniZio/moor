import type { BoxDetail } from './api';
import { nameFrom, openForm } from './WorkflowDraft';

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
