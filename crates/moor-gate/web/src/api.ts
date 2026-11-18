import * as v from 'valibot';

const SecretSchema = v.object({ name: v.string(), linked: v.boolean() });

const NodeSchema = v.object({
	name: v.string(),
	scope: v.picklist(['shared', 'personal']),
	runtime: v.string(),
	online: v.boolean(),
	offline_since: v.optional(v.number()),
	provides: v.optional(v.string()),
	used: v.number(),
	waiting: v.number(),
	max_boxes: v.optional(v.number()),
	mem_free_mib: v.optional(v.number()),
	mem_psi_centi: v.optional(v.number()),
	mem_report_age_secs: v.optional(v.number()),
	pressure_since: v.optional(v.number()),
	box_memory_mib: v.optional(v.number()),
	labels: v.array(v.string()),
	revision: v.optional(v.number(), 0),
	editable: v.optional(v.boolean(), false),
});

const AccountSchema = v.object({
	email: v.string(),
	csrf: v.string(),
	secrets: v.array(SecretSchema),
	machines: v.object({ nodes: v.array(NodeSchema), at: v.number() }),
	connect: v.object({ mcp_url: v.nullable(v.string()), server_url: v.string() }),
});

const RepoConfigSchema = v.object({
	repo: v.string(),
	text: v.nullish(v.string()),
	revision: v.number(),
	edited_by: v.nullish(v.string()),
	edited_at: v.nullish(v.string()),
	can_edit: v.boolean(),
});

const NodeSettingsSchema = v.object({
	name: v.string(),
	max_boxes: v.optional(v.number()),
	labels: v.array(v.string()),
	revision: v.number(),
	edited_by: v.nullish(v.string()),
	edited_at: v.nullish(v.string()),
});

const RepoListSchema = v.object({ repos: v.array(v.string()) });

const FailureSchema = v.object({ error: v.string() });

export type Secret = v.InferOutput<typeof SecretSchema>;
export type Node = v.InferOutput<typeof NodeSchema>;
export type RepoConfig = v.InferOutput<typeof RepoConfigSchema>;
export type NodeSettings = v.InferOutput<typeof NodeSettingsSchema>;
export type Account = v.InferOutput<typeof AccountSchema>;

/** Thrown when the session ended; the page offers sign-in again. */
export class SignedOut extends Error {}

/** Thrown when a save was made against a revision that is no longer the stored one. */
export class Conflict extends Error {}

/** The gate's answer parsed as `schema`, or why it is not one. */
async function answer<T extends v.GenericSchema>(res: Response, schema: T): Promise<v.InferOutput<T>> {
	if (res.status === 401) throw new SignedOut();
	const body: unknown = await res.json().catch(() => null);
	if (!res.ok) {
		const failure = v.safeParse(FailureSchema, body);
		const why = failure.success ? failure.output.error : `The gate answered ${res.status}.`;
		throw res.status === 409 ? new Conflict(why) : new Error(why);
	}
	return v.parse(schema, body);
}

export async function load(): Promise<Account> {
	return answer(await fetch('/account/api'), AccountSchema);
}

export async function link(csrf: string, name: string, value: string): Promise<Secret> {
	const res = await fetch(`/account/api/secrets/${encodeURIComponent(name)}`, {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ value }),
	});
	return answer(res, SecretSchema);
}

export async function signOut(csrf: string): Promise<void> {
	await fetch('/auth/logout', {
		method: 'POST',
		headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
		body: new URLSearchParams({ csrf }),
	});
}

/** `owner/name` as a URL path, each part encoded. */
function repoPath(repo: string): string {
	return `/account/api/repos/${repo.split('/').map(encodeURIComponent).join('/')}`;
}

export async function repoList(): Promise<string[]> {
	return (await answer(await fetch('/account/api/repos'), RepoListSchema)).repos;
}

export async function repoGet(repo: string): Promise<RepoConfig> {
	return answer(await fetch(repoPath(repo)), RepoConfigSchema);
}

/** Replaces the repo's config, against the revision the page read. */
export async function repoSet(csrf: string, repo: string, text: string, revision: number): Promise<RepoConfig> {
	const res = await fetch(repoPath(repo), {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ text, revision }),
	});
	return answer(res, RepoConfigSchema);
}

/** Replaces a node's cap (none for no cap) and labels, against the revision the page read. */
export async function nodeSet(
	csrf: string,
	name: string,
	maxBoxes: number | null,
	labels: string[],
	revision: number,
): Promise<NodeSettings> {
	const res = await fetch(`/account/api/nodes/${encodeURIComponent(name)}`, {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ max_boxes: maxBoxes, labels, revision }),
	});
	return answer(res, NodeSettingsSchema);
}

const StatusSchema = v.picklist(['queued', 'running', 'waiting', 'success', 'failure', 'stopped', 'skipped']);

const LastRunSchema = v.object({
	repo: v.string(),
	status: StatusSchema,
	label: v.string(),
	due_at: v.number(),
});

const WorkflowRowSchema = v.object({
	name: v.string(),
	repos: v.array(v.string()),
	cron: v.string(),
	timezone: v.string(),
	enabled: v.boolean(),
	disabled_reason: v.nullish(v.string()),
	next_due: v.nullish(v.number()),
	has_plan: v.boolean(),
	plan_stale: v.boolean(),
	last: v.nullish(LastRunSchema),
});

const WorkflowListSchema = v.object({ workflows: v.array(WorkflowRowSchema) });

const RunRowSchema = v.object({
	id: v.number(),
	repo: v.string(),
	due_at: v.number(),
	trigger: v.string(),
	started_at: v.nullish(v.number()),
	duration_ms: v.nullish(v.number()),
	status: StatusSchema,
	label: v.string(),
	result: v.nullish(v.string()),
	detail: v.nullish(v.string()),
});

const WorkflowSchema = v.object({
	...WorkflowRowSchema.entries,
	also: v.array(v.string()),
	timeout_minutes: v.number(),
	prompt: v.string(),
	plan: v.nullish(v.string()),
	plan_edited: v.boolean(),
	revision: v.number(),
	runs: v.array(RunRowSchema),
});

const LineSchema = v.object({
	at: v.number(),
	kind: v.string(),
	text: v.string(),
	evidence: v.array(v.object({ name: v.string(), href: v.nullish(v.string()) })),
});

const StepSchema = v.object({
	id: v.string(),
	title: v.string(),
	kind: v.string(),
	instructions: v.nullish(v.string()),
	done_when: v.nullish(v.string()),
	command: v.nullish(v.string()),
	question: v.nullish(v.string()),
	state: v.picklist(['pending', 'running', 'waiting', 'passed', 'failed', 'ended']),
	history: v.array(LineSchema),
	check: v.nullish(v.object({ exit: v.nullish(v.number()), tail: v.string() })),
	expects: v.array(v.object({ name: v.string() })),
});

const RunSchema = v.object({
	workflow: v.string(),
	id: v.number(),
	repo: v.string(),
	due_at: v.number(),
	started_at: v.nullish(v.number()),
	duration_ms: v.nullish(v.number()),
	phase: v.string(),
	status: StatusSchema,
	label: v.string(),
	result: v.nullish(v.string()),
	detail: v.nullish(v.string()),
	box: v.nullish(v.string()),
	question: v.nullish(v.object({ step: v.string(), text: v.string() })),
	steps: v.array(StepSchema),
	lines: v.array(LineSchema),
});

export type RunStatus = v.InferOutput<typeof StatusSchema>;
export type WorkflowRow = v.InferOutput<typeof WorkflowRowSchema>;
export type Workflow = v.InferOutput<typeof WorkflowSchema>;
export type RunRow = v.InferOutput<typeof RunRowSchema>;
export type Run = v.InferOutput<typeof RunSchema>;
export type Step = v.InferOutput<typeof StepSchema>;
export type Line = v.InferOutput<typeof LineSchema>;

const ClearsPlanSchema = v.object({ error: v.string(), code: v.literal('clears_plan') });

/** Thrown when a save would drop a stored plan; the page asks the person first. */
export class ClearsPlan extends Error {}

function workflowPath(name: string): string {
	return `/account/api/workflows/${encodeURIComponent(name)}`;
}

export async function workflowList(): Promise<WorkflowRow[]> {
	return (await answer(await fetch('/account/api/workflows'), WorkflowListSchema)).workflows;
}

export async function workflowGet(name: string): Promise<Workflow> {
	return answer(await fetch(workflowPath(name)), WorkflowSchema);
}

const RunNowSchema = v.object({ status: v.string(), box: v.nullable(v.string()) });

/** Fires the workflow once, now, for one repo. A repo with an open run is refused as a conflict. */
export async function workflowRunNow(csrf: string, name: string, repo: string) {
	const res = await fetch(`${workflowPath(name)}/run`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ repo }),
	});
	return answer(res, RunNowSchema);
}

/** Replaces a workflow's prompt and plan, against the revision the page read. */
export async function workflowSet(
	csrf: string,
	name: string,
	draft: { prompt: string; plan: string; revision: number; clearPlan: boolean },
): Promise<Workflow> {
	const res = await fetch(workflowPath(name), {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({
			prompt: draft.prompt,
			plan: draft.plan,
			revision: draft.revision,
			clear_plan: draft.clearPlan,
		}),
	});
	if (res.status === 400) {
		const body: unknown = await res.clone().json().catch(() => null);
		const clears = v.safeParse(ClearsPlanSchema, body);
		if (clears.success) throw new ClearsPlan(clears.output.error);
	}
	return answer(res, WorkflowSchema);
}

export async function runGet(name: string, id: number): Promise<Run> {
	return answer(await fetch(`${workflowPath(name)}/runs/${id}`), RunSchema);
}

/** Answers the approve step a run waits at. */
export async function runAnswer(csrf: string, name: string, id: number, yes: boolean): Promise<Run> {
	const res = await fetch(`${workflowPath(name)}/runs/${id}/answer`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ answer: yes ? 'yes' : 'no' }),
	});
	return answer(res, RunSchema);
}

const ProgressSchema = v.picklist(['queued', 'setting_up', 'working', 'needs_you', 'done', 'asleep', 'closed', 'failed']);

const PrSchema = v.object({ repo: v.string(), number: v.number(), url: v.string() });

const BoxRowSchema = v.object({
	box: v.string(),
	repo: v.nullish(v.string()),
	branch: v.nullish(v.string()),
	base: v.nullish(v.string()),
	prompt: v.nullish(v.string()),
	state: ProgressSchema,
	stale: v.boolean(),
	line: v.string(),
	last_event_at: v.nullish(v.string()),
	updated_at: v.string(),
	pr: v.nullable(PrSchema),
});

const EntryClassSchema = v.picklist(['message', 'step', 'you', 'asked', 'turn', 'evidence', 'error', 'routine', 'hidden']);

const EntrySchema = v.object({
	id: v.number(),
	at: v.string(),
	kind: v.string(),
	class: EntryClassSchema,
	text: v.string(),
});

const EvidenceSchema = v.object({
	name: v.string(),
	version: v.number(),
	kind: v.string(),
	content_type: v.string(),
	size: v.number(),
	created_at: v.string(),
	image: v.boolean(),
	href: v.string(),
});

const BoxDetailSchema = v.object({
	...BoxRowSchema.entries,
	timeline: v.array(EntrySchema),
	artifacts: v.array(EvidenceSchema),
});

export type Progress = v.InferOutput<typeof ProgressSchema>;
export type BoxRow = v.InferOutput<typeof BoxRowSchema>;
export type BoxDetail = v.InferOutput<typeof BoxDetailSchema>;
export type Entry = v.InferOutput<typeof EntrySchema>;
export type Evidence = v.InferOutput<typeof EvidenceSchema>;

export async function boxList(): Promise<BoxRow[]> {
	return (await answer(await fetch('/account/api/boxes'), v.object({ boxes: v.array(BoxRowSchema) }))).boxes;
}

/** Starts a Box as the signed-in person; answers the Box's name once it exists. */
export async function boxStart(
	csrf: string,
	body: { repo: string; prompt: string; base: string; node: string },
): Promise<string> {
	const res = await fetch('/account/api/boxes', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(body),
	});
	return (await answer(res, v.object({ box: v.string() }))).box;
}

export async function boxGet(box: string): Promise<BoxDetail> {
	return answer(await fetch(`/account/api/box?box=${encodeURIComponent(box)}`), BoxDetailSchema);
}
