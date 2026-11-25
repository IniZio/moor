import type { Workflow } from './api';
import { gist, leads, readPlan } from './plan';

type Making = Workflow['plan_making'];

/** Where moor stands in making the plan: at work in a Box, or stopped with a reason. */
function MakingNote({ making, busy, onAgain }: { making: Making; busy: boolean; onAgain: () => void }) {
	if (making?.failed) {
		return (
			<div class="notice danger" role="alert">
				<p>moor could not make a plan: {making.failed}</p>
				<button type="button" disabled={busy} onClick={onAgain}>
					Make the plan again
				</button>
			</div>
		);
	}
	if (making) {
		return (
			<p class="notice" role="status">
				moor is making the plan from the prompt.{' '}
				{making.box_name && <a href={`#/box/${encodeURIComponent(making.box_name)}`}>Watch</a>}
			</p>
		);
	}
	return null;
}

/** moor's plan of a workflow's prompt, read-only: a person changes the prompt, never the steps. */
export function PlanView({ flow, busy, onAgain }: { flow: Workflow; busy: boolean; onAgain: () => void }) {
	const making = flow.plan_making;
	const note = <MakingNote making={making} busy={busy} onAgain={onAgain} />;
	if (!flow.plan) {
		return (
			<section class="plan-view" aria-label="Plan">
				<h3>Plan</h3>
				{note}
				{!making && <p class="muted">No plan yet. Each run delivers the prompt in one step.</p>}
			</section>
		);
	}
	const read = readPlan(flow.plan);
	if ('error' in read) return <p class="notice danger" role="alert">{read.error}</p>;
	const waits = read.doc.steps.some((s) => s.kind === 'approve');
	return (
		<section class="plan-view" aria-label="Plan">
			<h3>Plan</h3>
			<p class="muted">
				moor's current reading of the prompt. It is made again when the prompt changes.{' '}
				{!making && (
					<button type="button" class="quiet" disabled={busy} onClick={onAgain}>
						Make it again
					</button>
				)}
			</p>
			{note}
			{flow.plan_stale && <p class="notice warn" role="status">This plan was made from an earlier prompt. It still runs until moor makes it again.</p>}
			{waits && flow.cron != null && <p class="notice warn" role="status">This plan has a step that waits for you, so a scheduled run can stop until you answer.</p>}
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
