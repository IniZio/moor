import { gist, leads, readPlan } from './plan';

/** moor's plan of a workflow's prompt, read-only: a person changes the prompt, never the steps. */
export function PlanView({ text, stale, cron }: { text: string | null | undefined; stale: boolean; cron: boolean }) {
	if (!text) {
		return (
			<section class="plan-view" aria-label="Plan">
				<h3>Plan</h3>
				<p class="muted">No plan yet. Each run delivers the prompt in one step.</p>
			</section>
		);
	}
	const read = readPlan(text);
	if ('error' in read) return <p class="notice danger" role="alert">{read.error}</p>;
	const waits = read.doc.steps.some((s) => s.kind === 'approve');
	return (
		<section class="plan-view" aria-label="Plan">
			<h3>Plan</h3>
			<p class="muted">moor's current reading of the prompt. It is made again when the prompt changes.</p>
			{stale && <p class="notice warn" role="status">This plan was made from an earlier prompt. It still runs until moor makes it again.</p>}
			{waits && cron && <p class="notice warn" role="status">This plan has a step that waits for you, so a scheduled run can stop until you answer.</p>}
			<ol>
				{read.doc.steps.map((s) => (
					<li key={s.id}>
						<strong>{s.title}</strong> <span class="muted">{s.kind}</span>
						<div class="muted">{gist(s)}</div>
						<div class="muted">{leads(read.doc, s)}</div>
					</li>
				))}
			</ol>
		</section>
	);
}
