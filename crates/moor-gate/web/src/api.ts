import * as v from 'valibot';

import { GateDocSchema, GateViewSchema, OutcomeDocSchema, plain, UnansweredSchema, VerdictSchema, WaitSchema } from './gate';
import { announceSignedOut } from './session';

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
	/** The agent profile repo, `https://github.com/OWNER/NAME[#REF]`, when one is set. */
	profile: v.nullish(v.string()),
	/** Whether the person's sends are questions an agent place answers, with no work Box. */
	asks: v.optional(v.boolean(), false),
});

const RulesSeenSchema = v.object({
	days: v.number(),
	withheld: v.array(v.object({ request: v.string(), host: v.string(), boxes: v.number() })),
	dialled: v.array(v.object({ host: v.string(), port: v.number(), count: v.number() })),
});

const RepoConfigSchema = v.object({
	seen: v.nullish(RulesSeenSchema),
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
export type RulesSeen = v.InferOutput<typeof RulesSeenSchema>;
export type NodeSettings = v.InferOutput<typeof NodeSettingsSchema>;
export type Account = v.InferOutput<typeof AccountSchema>;

/** Thrown when the session ended; the page offers sign-in again. */
export class SignedOut extends Error {}

/** What a failed call says. Empty when the session ended, because the sign-in dialog already says so. */
export function failure(e: Error): string {
	return e instanceof SignedOut ? '' : e.message;
}

/** What a catch caught, as an Error: the one place a thrown value of unknown type is read. */
export function toError(e: unknown): Error {
	return e instanceof Error ? e : new Error('Something went wrong.');
}

/** Thrown when a save was made against a revision that is no longer the stored one. */
export class Conflict extends Error {}

/** The gate's answer parsed as `schema`, or why it is not one. */
async function answer<T extends v.GenericSchema>(res: Response, schema: T): Promise<v.InferOutput<T>> {
	if (res.status === 401) {
		announceSignedOut();
		throw new SignedOut();
	}
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

/** Which of a repo's texts in moor: its config, or the Proxy rules its Boxes follow. */
export type RepoFile = 'config' | 'rules';

/** `owner/name` as a URL path, each part encoded, with `/rules` for the rules. */
function repoPath(repo: string, file: RepoFile): string {
	const path = `/account/api/repos/${repo.split('/').map(encodeURIComponent).join('/')}`;
	return file === 'rules' ? `${path}/rules` : path;
}

export async function repoList(): Promise<string[]> {
	return (await answer(await fetch('/account/api/repos'), RepoListSchema)).repos;
}

export async function repoGet(repo: string, file: RepoFile = 'config'): Promise<RepoConfig> {
	return answer(await fetch(repoPath(repo, file)), RepoConfigSchema);
}

/** Replaces the repo's config or rules, against the revision the page read. */
export async function repoSet(csrf: string, repo: string, text: string, revision: number, file: RepoFile = 'config'): Promise<RepoConfig> {
	const res = await fetch(repoPath(repo, file), {
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
	status: StatusSchema,
	label: v.string(),
	/** What the run found, when it names it: a bad one such as "check failed" is still a delivered run. */
	verdict: v.nullish(VerdictSchema),
	due_at: v.number(),
});

const WorkflowRowSchema = v.object({
	name: v.string(),
	repos: v.array(v.string()),
	/** None for a workflow with no cron, which runs only by hand or on pull requests. */
	cron: v.nullish(v.string()),
	/** The schedule in words, when it is one of the shapes the form offers, or what fires a workflow with no cron. */
	words: v.nullish(v.string()),
	timezone: v.nullish(v.string()),
	enabled: v.boolean(),
	/** Whether a pull request event on the first repo fires a run. */
	on_pull_request: v.optional(v.boolean(), false),
	next_due: v.nullish(v.number()),
	/** The next run in the workflow's own zone, such as "Fri 20 Nov, 08:30 (Asia/Hong_Kong)". */
	next_text: v.nullish(v.string()),
	has_plan: v.boolean(),
	plan_stale: v.boolean(),
	last: v.nullish(LastRunSchema),
});

const WorkflowListSchema = v.object({ workflows: v.array(WorkflowRowSchema) });

const RunRowSchema = v.object({
	id: v.number(),
	due_at: v.number(),
	trigger: v.string(),
	/** The pull request the run is for, `OWNER/NAME#NUMBER`. */
	pull: v.nullish(v.string()),
	/** The pull request's head commit the run is for. */
	head_sha: v.nullish(v.string()),
	started_at: v.nullish(v.number()),
	duration_ms: v.nullish(v.number()),
	status: StatusSchema,
	label: v.string(),
	verdict: v.nullish(VerdictSchema),
	result: v.nullish(v.string()),
	detail: v.nullish(v.string()),
});

const WorkflowSchema = v.object({
	...WorkflowRowSchema.entries,
	timeout_minutes: v.number(),
	prompt: v.string(),
	plan: v.nullish(v.string()),
	/** The plan moor is making from the prompt, or why it made none. */
	plan_making: v.nullish(v.object({ prompt: v.string(), box_name: v.nullish(v.string()), since: v.nullish(v.number()), failed: v.nullish(v.string()) })),
	/** The gates each run is held to, when the workflow has them in place of a plan. */
	gates: v.nullish(v.array(GateDocSchema)),
	/** Who wrote the gates: a person (kept until moor is asked to make its reading again) or moor. */
	gates_by: v.nullish(v.picklist(['person', 'moor'])),
	/** The outcomes its runs may end in, when it names them. */
	outcomes: v.nullish(v.array(OutcomeDocSchema)),
	revision: v.number(),
	runs: v.array(RunRowSchema),
	/** Who each run acts as. */
	run_as: v.optional(v.string(), ''),
	/** The extra Proxy entries the workflow's config asks for, rule to entries as written, or why the config does not parse. */
	may: v.optional(v.object({ allow: v.optional(v.record(v.string(), v.array(v.string()))), error: v.optional(v.string()) }), {}),
});

const LineSchema = v.object({
	at: v.number(),
	kind: v.string(),
	text: v.string(),
	evidence: v.array(v.object({ name: v.string(), href: v.nullish(v.string()) })),
	unanswered: v.nullish(UnansweredSchema),
});

const StepSchema = v.object({
	id: v.string(),
	title: v.string(),
	kind: v.string(),
	instructions: v.nullish(v.string()),
	done_when: v.nullish(v.string()),
	command: v.nullish(v.string()),
	/** A gate's checks, each with its sentence when it has one. */
	checks: v.nullish(v.array(v.object({ run: v.string(), says: v.nullish(v.string()) }))),
	question: v.nullish(v.string()),
	state: v.picklist(['pending', 'running', 'waiting', 'passed', 'failed', 'ended']),
	history: v.array(LineSchema),
	check: v.nullish(v.object({ exit: v.nullish(v.number()), tail: v.string() })),
	expects: v.array(v.object({ name: v.string() })),
});

const RunSchema = v.object({
	workflow: v.string(),
	id: v.number(),
	due_at: v.number(),
	started_at: v.nullish(v.number()),
	duration_ms: v.nullish(v.number()),
	phase: v.string(),
	outcome: v.nullish(v.string()),
	status: StatusSchema,
	label: v.string(),
	verdict: v.nullish(VerdictSchema),
	/** Where the run stands at its closing stage, while it has no outcome yet. */
	closing: v.nullish(v.picklist(['deciding', 'waiting'])),
	/** The pull request the run is for, `OWNER/NAME#NUMBER`. */
	pull: v.nullish(v.string()),
	result: v.nullish(v.string()),
	detail: v.nullish(v.string()),
	box: v.nullish(v.string()),
	question: v.nullish(v.object({ step: v.string(), text: v.string() })),
	steps: v.array(StepSchema),
	lines: v.array(LineSchema),
	/** Whether the reader owns the run's workflow, so may answer and fire it. */
	yours: v.optional(v.boolean(), true),
});

export type RunStatus = v.InferOutput<typeof StatusSchema>;
export type WorkflowRow = v.InferOutput<typeof WorkflowRowSchema>;
export type Workflow = v.InferOutput<typeof WorkflowSchema>;
export type RunRow = v.InferOutput<typeof RunRowSchema>;
export type Run = v.InferOutput<typeof RunSchema>;
export type Step = v.InferOutput<typeof StepSchema>;
export type Line = v.InferOutput<typeof LineSchema>;

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

/** Fires the workflow once, now, in one Box with all its repos, for pull request `pull` at its current head when given. A workflow with an open run is refused as a conflict. */
export async function workflowRunNow(csrf: string, name: string, pull?: string | null) {
	const res = await fetch(`${workflowPath(name)}/run`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(pull ? { pull } : {}),
	});
	return answer(res, RunNowSchema);
}

/** Replaces a workflow's prompt, against the revision the page read. moor makes its plan. */
export async function workflowSet(csrf: string, name: string, draft: { prompt: string; revision: number; regenerate?: boolean }): Promise<Workflow> {
	const res = await fetch(workflowPath(name), {
		method: 'PUT',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ prompt: draft.prompt, revision: draft.revision, regenerate: draft.regenerate ?? false }),
	});
	return answer(res, WorkflowSchema);
}

/** What the workflow form sets. */
export type WorkflowFields = { repos: string[]; prompt: string; cron: string; timezone: string; timeout_minutes?: number; on_pull_request?: boolean };

type Body = Record<string, string | number | boolean | string[] | undefined>;

function send(csrf: string, method: string, path: string, body: Body) {
	return fetch(path, {
		method,
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(body),
	});
}

/** Makes a workflow. A taken name is refused as a conflict; `copyOf` lends its config. */
export async function workflowCreate(csrf: string, name: string, fields: WorkflowFields, copyOf?: string): Promise<Workflow> {
	return answer(await send(csrf, 'POST', '/account/api/workflows', { name, ...fields, copy_of: copyOf }), WorkflowSchema);
}

/** Replaces a workflow's fields, against the revision the page read; its plan stays. */
export async function workflowEdit(csrf: string, name: string, fields: WorkflowFields, revision: number): Promise<Workflow> {
	return answer(await send(csrf, 'PUT', `${workflowPath(name)}/fields`, { ...fields, revision }), WorkflowSchema);
}

export async function workflowSwitch(csrf: string, name: string, enabled: boolean): Promise<Workflow> {
	return answer(await send(csrf, 'POST', `${workflowPath(name)}/enabled`, { enabled }), WorkflowSchema);
}

export async function workflowRemove(csrf: string, name: string): Promise<void> {
	await answer(await send(csrf, 'DELETE', workflowPath(name), {}), v.object({ removed: v.string() }));
}

const PreviewSchema = v.object({ words: v.nullish(v.string()), next: v.array(v.number()) });

/** The schedule in words and its next three firings, as the controller reads it. */
export async function schedulePreview(cron: string, timezone: string) {
	const query = new URLSearchParams({ cron, timezone });
	return answer(await fetch(`/account/api/schedule?${query}`), PreviewSchema);
}

/** Run `id` of anyone's workflow, which any signed-in person may read. */
export async function runRead(id: number): Promise<Run> {
	return answer(await fetch(`/account/api/runs/${id}`), RunSchema);
}

export async function runGet(name: string, id: number): Promise<Run> {
	return answer(await fetch(`${workflowPath(name)}/runs/${id}`), RunSchema);
}

/** Answers the approve step a run waits at. */
export async function runAnswer(csrf: string, name: string, id: number, yes: boolean, note?: string): Promise<Run> {
	const res = await fetch(`${workflowPath(name)}/runs/${id}/answer`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ answer: yes ? 'yes' : 'no', note: note?.trim() || undefined }),
	});
	return answer(res, RunSchema);
}

const ProgressSchema = v.picklist(['queued', 'setting_up', 'working', 'needs_you', 'done', 'asleep', 'closed', 'failed']);

const PrSchema = v.object({ repo: v.string(), number: v.number(), url: v.string() });

const PickSchema = v.object({
	n: v.number(),
	label: v.string(),
	role: v.picklist(['allow_once', 'allow_box', 'deny', 'choice']),
});

/** What the agent asks, as the controller derives it: the card a person answers. */
const NeedsSchema = v.object({
	id: v.string(),
	kind: v.picklist(['permission', 'question', 'approve', 'stuck']),
	question: v.string(),
	detail: v.nullish(v.string()),
	picks: v.array(PickSchema),
	other: v.nullish(v.number()),
	/** For an approve card: the workflow run its yes or no goes to. */
	approve: v.nullish(v.object({ workflow: v.string(), run: v.number(), step: v.string() })),
	/** Whether a plan-first agent asks to approve the plan it left as evidence. */
	plan: v.optional(v.boolean(), false),
	/** For the card of a gate that waits for a person: the gate and what the person needs to read to act. */
	gate: v.nullish(WaitSchema),
});

/** The words of a row a person reads. */
type Spoken = {
	line: string;
	gate?: { gates: string[] } | null;
	needs?: { question: string; detail?: string | null; gate?: { hint?: string | null; note?: string | null } | null } | null;
};

/** A row with the code marks taken out of the sentences moor wrote for a person; the gates it names are quoted as the page quotes them. */
function spoken<T extends Spoken>(row: T): T {
	const say = (text: string) => plain(text, row.gate?.gates ?? []);
	row.line = say(row.line);
	const { needs } = row;
	if (needs) {
		needs.question = say(needs.question);
		if (needs.detail) needs.detail = say(needs.detail);
		if (needs.gate?.hint) needs.gate.hint = say(needs.gate.hint);
		if (needs.gate?.note) needs.gate.note = say(needs.gate.note);
	}
	return row;
}

/** The workflow whose run started a Box, and the pull request that run was for. */
const WorkflowRefSchema = v.object({ name: v.string(), pull: v.nullish(v.string()) });

const BoxRowFields = v.object({
	box: v.string(),
	repo: v.nullish(v.string()),
	/** The repos cloned beside the first, in order. */
	other_repos: v.optional(v.array(v.string()), []),
	branch: v.nullish(v.string()),
	base: v.nullish(v.string()),
	prompt: v.nullish(v.string()),
	/** The issue the Box was started for, as the person wrote it. */
	issue: v.nullish(v.string()),
	/** The name of the node the Box runs on; none while it is placed on none. */
	machine: v.nullish(v.string()),
	workflow: v.nullish(WorkflowRefSchema),
	title: v.string(),
	state: ProgressSchema,
	stale: v.boolean(),
	line: v.string(),
	last_event_at: v.nullish(v.string()),
	updated_at: v.string(),
	pr: v.nullable(PrSchema),
	needs: v.nullish(NeedsSchema),
	/** The gates the Box must pass before it counts as done, and where it stands. */
	gate: v.nullish(GateViewSchema),
	/** Whether the Box failed, as moor-core decides: the one answer every view reads. */
	failed: v.optional(v.boolean(), false),
	/** Whether setup failed, so it can be run again. */
	retryable: v.optional(v.boolean(), false),
	/** Who the task is for, when it is another person's: read-only here. */
	owner: v.nullish(v.string()),
	/** When the task was asked, for a question's task, which has no work Box behind it. */
	task_at: v.nullish(v.string()),
});

const BoxRowSchema = v.pipe(BoxRowFields, v.transform(spoken));

const TallySchema = v.object({ files: v.number(), added: v.number(), removed: v.number() });

/** What a turn changed in git, read by moor once the turn ended. */
const ReceiptSchema = v.object({ turn: v.number(), changed: TallySchema, total: TallySchema });

const EntryClassSchema = v.picklist(['message', 'step', 'you', 'moor', 'asked', 'turn', 'evidence', 'error', 'routine', 'hidden']);

/** A text moor kept of a tool call, cut to size; `cut` when part of it was left out. */
const BodySchema = v.object({ text: v.string(), cut: v.optional(v.boolean(), false) });

/** A tool call read from the agent's saved transcript. */
const ToolSchema = v.object({
	name: v.string(),
	title: v.string(),
	/** For a routine call: what it folds with. */
	group: v.nullish(v.picklist(['read', 'search', 'plan'])),
	/** It failed, exited non-zero or was refused: never folded. */
	failed: v.optional(v.boolean(), false),
	/** A report moor did not accept: normal flow, shown plain and not as an error. */
	refused: v.optional(v.boolean(), false),
	input: v.nullish(BodySchema),
	diff: v.nullish(BodySchema),
	output: v.nullish(BodySchema),
});

const EntrySchema = v.object({
	id: v.number(),
	/** For a line of the agent's transcript: its id there. */
	key: v.nullish(v.string()),
	tool: v.nullish(ToolSchema),
	at: v.string(),
	kind: v.string(),
	class: EntryClassSchema,
	text: v.string(),
	/** For a workflow run's first message: the title of the step it starts at. */
	step: v.nullish(v.string()),
	/** For moor's message that carries a person's words, such as the owner's note in a no: those words, which the text leaves out. */
	quote: v.nullish(v.string()),
	/** For a turn: what it changed in git. */
	receipt: v.nullish(ReceiptSchema),
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

/** A work Box the place of a question's task started: what the task page says of it in one line, and the files it left. A state this page does not know is shown as sent. */
const TaskBoxSchema = v.object({
	/** The work Box's id; a later Box of the task may reuse its name. */
	id: v.number(),
	name: v.string(),
	repo: v.string(),
	started_at: v.string(),
	/** Milliseconds from start until the Box was ready; none before it is. */
	ready_ms: v.nullish(v.number()),
	state: v.string(),
	closed_at: v.nullish(v.string()),
	artifacts: v.optional(v.array(EvidenceSchema), []),
});

const BoxDetailFields = v.object({
	...BoxRowFields.entries,
	timeline: v.array(EntrySchema),
	artifacts: v.array(EvidenceSchema),
	/** The workflow and pull request of the failed run this Box came from, when it is that workflow's latest run. */
	run_again: v.nullish(WorkflowRefSchema),
	/** Whether the person asked for a plan first; its approved plan is the newest plan.json. */
	plan_first: v.optional(v.boolean(), false),
	/** Whether a workflow run made this Box. */
	from_workflow: v.optional(v.boolean(), false),
	/** The work Boxes a question's place started, oldest first; none from a controller that predates them. */
	task_boxes: v.optional(v.array(TaskBoxSchema), []),
});

const BoxDetailSchema = v.pipe(BoxDetailFields, v.transform(spoken));

export type Needs = v.InferOutput<typeof NeedsSchema>;
export type Pick = v.InferOutput<typeof PickSchema>;
export type Progress = v.InferOutput<typeof ProgressSchema>;
export type BoxRow = v.InferOutput<typeof BoxRowSchema>;
export type BoxDetail = v.InferOutput<typeof BoxDetailSchema>;
export type Entry = v.InferOutput<typeof EntrySchema>;
export type Tool = v.InferOutput<typeof ToolSchema>;
export type Clipped = v.InferOutput<typeof BodySchema>;
export type Evidence = v.InferOutput<typeof EvidenceSchema>;
export type TaskBox = v.InferOutput<typeof TaskBoxSchema>;
export type Tally = v.InferOutput<typeof TallySchema>;

/** The person's open tasks, or every person's with `everyone`. */
export async function boxList(everyone = false): Promise<BoxRow[]> {
	const path = everyone ? '/account/api/boxes?who=everyone' : '/account/api/boxes';
	return (await answer(await fetch(path), v.object({ boxes: v.array(BoxRowSchema) }))).boxes;
}

/** Starts a Box as the signed-in person; answers the Box's name once it exists. */
export async function boxStart(
	csrf: string,
	body: { repos: string[]; prompt: string; base: string; node: string; plan_first?: boolean },
): Promise<string> {
	const res = await fetch('/account/api/boxes', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(body),
	});
	return (await answer(res, v.object({ box: v.string() }))).box;
}

const InboxItemSchema = v.object({
	id: v.number(),
	box: v.string(),
	repo: v.nullish(v.string()),
	kind: v.picklist(['needs_input', 'run_waiting', 'run_failed']),
	state: v.picklist(['unread', 'read', 'resolved']),
	title: v.pipe(v.string(), v.transform((s) => plain(s))),
	text: v.pipe(v.string(), v.transform((s) => plain(s))),
	at: v.string(),
});

const InboxSchema = v.object({ items: v.array(InboxItemSchema), unread: v.number() });

export type InboxItem = v.InferOutput<typeof InboxItemSchema>;
export type Inbox = v.InferOutput<typeof InboxSchema>;

/** The person's inbox, newest first, with how many items are unread. */
export async function inboxList(): Promise<Inbox> {
	return answer(await fetch('/account/api/inbox'), InboxSchema);
}

/** Marks inbox items read, or every item when `ids` is empty; answers the inbox as it is then. */
export async function inboxRead(csrf: string, ids: number[]): Promise<Inbox> {
	const res = await fetch('/account/api/inbox/read', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ ids }),
	});
	return answer(res, InboxSchema);
}

/** Takes the items `ids` names out of the inbox, or every item up to `through`; answers the inbox as it is then. */
export async function inboxClear(csrf: string, which: { ids: number[] } | { through: number }): Promise<Inbox> {
	const res = await fetch('/account/api/inbox/clear', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(which),
	});
	return answer(res, InboxSchema);
}

/** Runs setup again on a Box whose setup failed. */
export async function boxRetry(csrf: string, box: string): Promise<void> {
	const res = await fetch('/account/api/box/retry', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ box }),
	});
	await answer(res, v.object({ box: v.string() }));
}

/** Answers what a Box's agent asks: menu entry `n` of the prompt named `ask`. */
export async function boxAnswer(csrf: string, box: string, ask: string, n: number, text?: string): Promise<void> {
	const res = await fetch('/account/api/box/answer', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ box, ask, n, text }),
	});
	await answer(res, v.object({ answered: v.boolean() }));
}

/** Answers the yes or no a gate asks its owner, for the prompt `ask` names; a note goes to the agent. Says what the answer did. */
export async function gateAnswer(csrf: string, box: string, ask: string, yes: boolean, note?: string): Promise<string | undefined> {
	const res = await fetch('/account/api/box/answer', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify({ box, ask, answer: yes ? 'yes' : 'no', note: note?.trim() || undefined }),
	});
	return (await answer(res, v.object({ answered: v.boolean(), outcome: v.nullish(v.string()) }))).outcome ?? undefined;
}

const FileChangeSchema = v.object({
	path: v.string(),
	from: v.nullish(v.string()),
	/** Null for a binary file. */
	added: v.nullable(v.number()),
	removed: v.nullable(v.number()),
});

const RepoChangesSchema = v.object({
	repo: v.string(),
	total: TallySchema,
	files: v.array(FileChangeSchema),
	/** The unified diff against the base; absent unless the Box was read now. */
	patch: v.nullish(v.string()),
	cut: v.optional(v.boolean(), false),
	problem: v.nullish(v.string()),
});

const ChangesSchema = v.object({
	live: v.boolean(),
	why: v.nullish(v.string()),
	at: v.nullish(v.string()),
	repos: v.array(RepoChangesSchema),
});

export type FileChange = v.InferOutput<typeof FileChangeSchema>;
export type RepoChanges = v.InferOutput<typeof RepoChangesSchema>;
export type Changes = v.InferOutput<typeof ChangesSchema>;

/** A Box's changes in git against its base: read now when it is awake, else the diff kept when it last ended a turn or slept. */
export async function boxChanges(box: string): Promise<Changes> {
	return answer(await fetch(`/account/api/box/changes?box=${encodeURIComponent(box)}`), ChangesSchema);
}

const RepoPullSchema = v.object({
	repo: v.string(),
	state: v.picklist(['unpushed', 'none', 'draft', 'open', 'changes_requested', 'approved', 'merged', 'closed']),
	number: v.nullable(v.number()),
	checks: v.nullable(v.picklist(['passing', 'failing', 'pending'])),
	/** The pull request's page, or before there is one, the page that opens it; none until the branch is pushed. */
	url: v.nullable(v.string()),
	/** Why GitHub could not be read; then the state says nothing. */
	unread: v.nullish(v.string()),
});

const PullsSchema = v.object({ branch: v.nullable(v.string()), repos: v.array(RepoPullSchema) });

export type RepoPull = v.InferOutput<typeof RepoPullSchema>;

/** The pull request on each of a Box's repos, first repo first, as GitHub says now. */
export async function boxPulls(box: string): Promise<RepoPull[]> {
	return (await answer(await fetch(`/account/api/box/pr?box=${encodeURIComponent(box)}`), PullsSchema)).repos;
}

export async function boxGet(box: string): Promise<BoxDetail> {
	return answer(await fetch(`/account/api/box?box=${encodeURIComponent(box)}`), BoxDetailSchema);
}
