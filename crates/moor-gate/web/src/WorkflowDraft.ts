import { type Workflow } from './api';
import { browserZone, FIRST, fromCron, type Schedule } from './schedule';

/** What the form opens with: a duplicate, a one-off made recurring, or nothing. */
export type Draft = { name: string; repos: string[]; prompt: string; schedule: Schedule; timezone: string; timeout: string; onPull: boolean; copyOf?: string };

const DRAFT = 'moor.workflowDraft';

/** A valid workflow name made from any text. */
export function nameFrom(text: string): string {
	const slug = text.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[^a-z0-9]+/, '').slice(0, 64).replace(/-+$/, '');
	return slug || 'workflow';
}

export function openForm(draft: Partial<Draft>) {
	sessionStorage.setItem(DRAFT, JSON.stringify(draft));
	location.hash = '#/new-workflow';
}

export function takeDraft(): Partial<Draft> {
	const raw = sessionStorage.getItem(DRAFT);
	sessionStorage.removeItem(DRAFT);
	try {
		return raw ? (JSON.parse(raw) as Partial<Draft>) : {};
	} catch {
		return {};
	}
}

export function fromFlow(flow: Workflow): Draft {
	return {
		name: flow.name,
		repos: flow.repos,
		prompt: flow.prompt,
		schedule: fromCron(flow.cron),
		timezone: flow.timezone ?? browserZone(),
		timeout: String(flow.timeout_minutes),
		onPull: flow.on_pull_request,
	};
}

export function blank(draft: Partial<Draft>): Draft {
	return { name: '', repos: [], prompt: '', schedule: FIRST, timezone: browserZone(), timeout: '', onPull: false, ...draft };
}
