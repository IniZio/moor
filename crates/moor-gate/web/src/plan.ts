import * as v from 'valibot';

export const KINDS = ['work', 'check', 'approve', 'deliver'] as const;
export type Kind = (typeof KINDS)[number];

const text = v.nullish(v.string());
const StepDocSchema = v.strictObject({
	id: v.string(),
	title: v.string(),
	kind: v.string(),
	max_tries: v.nullish(v.pipe(v.number(), v.integer(), v.minValue(0))),
	instructions: text,
	done_when: text,
	evidence: v.nullish(v.array(v.string())),
	command: text,
	question: text,
	ok: text,
	fail: text,
});
const PlanDocSchema = v.strictObject({
	steps: v.array(StepDocSchema),
	made_from: text,
	edited: v.nullish(v.boolean()),
});

export type StepDoc = {
	id: string;
	title: string;
	kind: string;
	max_tries?: number;
	instructions?: string;
	done_when?: string;
	evidence?: string[];
	command?: string;
	question?: string;
	ok?: string;
	fail?: string;
};
export type PlanDoc = { steps: StepDoc[]; made_from?: string; edited?: boolean };

/** One thing wrong with a plan, worded as the controller words it, with the step it belongs to. */
export type Problem = { step: number | null; text: string };

const STEP_KEYS = ['id', 'title', 'kind', 'max_tries', 'instructions', 'done_when', 'evidence', 'command', 'question', 'ok', 'fail'] as const;
const KIND_FIELDS: Record<Kind, readonly (keyof StepDoc)[]> = {
	work: ['instructions', 'done_when', 'evidence', 'ok', 'fail'],
	check: ['command', 'ok', 'fail'],
	approve: ['question', 'ok', 'fail'],
	deliver: ['instructions', 'done_when'],
};
const OPTIONAL: readonly (keyof StepDoc)[] = ['instructions', 'done_when', 'evidence', 'command', 'question', 'ok', 'fail'];
export const MAX_STEPS = 20;
const MAX_TITLE = 120;
const MAX_TEXT = 4096;

export function isKind(kind: string): kind is Kind {
	return (KINDS as readonly string[]).includes(kind);
}

function dropNulls<T extends object>(o: T): T {
	return Object.fromEntries(Object.entries(o).filter(([, x]) => x != null)) as T;
}

/** The plan in `text`, or why it is not one. */
export function readPlan(text: string): { doc: PlanDoc } | { error: string } {
	let raw: unknown;
	try {
		raw = JSON.parse(text);
	} catch (err) {
		return { error: `the plan is not valid: ${err instanceof Error ? err.message : 'not JSON'}` };
	}
	if (!v.is(v.looseObject({}), raw) || Array.isArray(raw)) {
		return { error: 'the plan is not valid: a plan is an object with a `steps` list, like {"steps": [...]}' };
	}
	const parsed = v.safeParse(PlanDocSchema, raw);
	if (!parsed.success) {
		const issue = parsed.issues[0];
		const keyed = issue.type === 'strict_object';
		const at = (keyed ? issue.path?.slice(0, -1) : issue.path)?.map((p) => String(p.key)).join('.');
		const why = !keyed ? issue.message : issue.received === 'undefined' ? `missing field ${issue.expected}` : `unknown field ${issue.received}`;
		return { error: `the plan is not valid: ${at ? `${at}: ` : ''}${why}` };
	}
	const doc = dropNulls(parsed.output) as PlanDoc;
	return { doc: { ...doc, steps: parsed.output.steps.map((s) => dropNulls(s) as StepDoc) } };
}

/** The plan as the JSON text the controller keeps, keys in its order. */
export function planText(doc: PlanDoc): string {
	const steps = doc.steps.map((s) => Object.fromEntries(STEP_KEYS.flatMap((k) => (s[k] === undefined ? [] : [[k, s[k]]]))));
	return JSON.stringify({ steps, made_from: doc.made_from, edited: doc.edited || undefined }, null, 2);
}

/** `text` written the way `planText` writes it, so two plans compare by content. */
export function sameText(text: string): string {
	const read = text.trim() === '' ? null : readPlan(text);
	return read && 'doc' in read ? planText(read.doc) : text.trim();
}

/** The plan a workflow with none starts from: one deliver step that delivers the prompt. */
export function starter(prompt: string): PlanDoc {
	return {
		steps: [{ id: 'deliver', title: 'Deliver', kind: 'deliver', instructions: prompt, done_when: "The prompt's work is done." }],
	};
}

const bytes = (s: string) => new TextEncoder().encode(s).length;
const validId = (s: string) => /^[a-z][a-z0-9-]{0,31}$/.test(s);
const validEvidence = (s: string) => /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(s.endsWith('*') ? s.slice(0, -1) : s);

/** Every problem the controller's Plan::parse would find, keyed by step. */
export function problems(doc: PlanDoc): Problem[] {
	const out: Problem[] = [];
	const plan = (text: string) => out.push({ step: null, text });
	if (doc.made_from !== undefined && !/^[0-9a-fA-F]{64}$/.test(doc.made_from)) plan('made_from must be 64 hex digits');
	if (doc.steps.length === 0) plan('a plan needs at least one step');
	if (doc.steps.length > MAX_STEPS) plan(`a plan has ${doc.steps.length} steps; use at most ${MAX_STEPS}`);
	const ids = new Set(doc.steps.map((s) => s.id));
	const seen = new Set<string>();
	doc.steps.forEach((s, i) => {
		const say = (text: string) => out.push({ step: i, text });
		if (seen.has(s.id)) say(`step id \`${s.id}\` is used twice`);
		seen.add(s.id);
		let place = `step \`${s.id}\``;
		if (!validId(s.id)) {
			say(`step ${i + 1} (\`${s.id}\`): invalid step id; use 1-32 lowercase letters, digits or '-', starting with a letter`);
			place = `step ${i + 1}`;
		}
		const len = bytes(s.title);
		if (s.title.trim() === '' || len > MAX_TITLE) say(`${place}: the title must be 1 to 120 bytes, not ${len}`);
		if (s.max_tries !== undefined && (s.max_tries < 1 || s.max_tries > 5)) say(`${place}: max_tries must be 1 to 5, not ${s.max_tries}`);
		if (!isKind(s.kind)) return void say(`${place}: unknown kind \`${s.kind}\`; use work, check, approve or deliver`);
		const kind = s.kind;
		const takes = KIND_FIELDS[kind];
		for (const field of OPTIONAL) {
			if (!takes.includes(field) && s[field] !== undefined) say(`${place}: a ${kind} step takes no \`${field}\``);
		}
		for (const field of ['instructions', 'done_when', 'command', 'question'] as const) {
			if (!takes.includes(field)) continue;
			const value = s[field];
			if (value === undefined || value.trim() === '') say(`${place}: a ${kind} step needs \`${field}\``);
			else if (bytes(value) > MAX_TEXT) say(`${place}: \`${field}\` is ${bytes(value)} bytes; use at most 4096`);
		}
		if (kind === 'work') {
			if (!s.evidence?.length) say(`${place}: a work step needs \`evidence\``);
			for (const name of s.evidence ?? []) {
				if (!validEvidence(name)) say(`${place}: evidence \`${name}\` is not a flat file name or a \`prefix*\``);
			}
		}
		if (kind === 'deliver') return;
		if (s.ok === undefined) say(`${place}: a ${kind} step needs \`ok\``);
		for (const edge of ['ok', 'fail'] as const) {
			const target = s[edge];
			if (target !== undefined && !(validId(target) && ids.has(target))) {
				say(`${place}: \`${edge}\` names \`${target}\`, which is not a step of the plan`);
			}
		}
	});
	if (doc.steps.length > 0 && doc.steps.every((s) => s.kind !== 'deliver')) plan('a plan needs a deliver step');
	return out.length > 0 ? out : trapped(doc);
}

/** The steps from which no deliver step can be reached. */
function trapped(doc: PlanDoc): Problem[] {
	const reached = new Set(doc.steps.flatMap((s) => (s.kind === 'deliver' ? [s.id] : [])));
	for (let grew = true; grew; ) {
		grew = false;
		for (const s of doc.steps) {
			if (!reached.has(s.id) && [s.ok, s.fail].some((t) => t !== undefined && reached.has(t))) {
				reached.add(s.id);
				grew = true;
			}
		}
	}
	return doc.steps.flatMap((s, i) =>
		reached.has(s.id) ? [] : [{ step: i, text: `step \`${s.id}\`: no deliver step can be reached from here, so a run could never finish` }],
	);
}

/** Every problem with the plan text, or none when it is blank: blank means no plan. */
export function textProblems(text: string): Problem[] {
	if (text.trim() === '') return [];
	const read = readPlan(text);
	return 'doc' in read ? problems(read.doc) : [{ step: null, text: read.error }];
}

function blank(kind: Kind): Partial<StepDoc> {
	return Object.fromEntries(KIND_FIELDS[kind].flatMap((f) => (f === 'ok' || f === 'fail' ? [] : [[f, f === 'evidence' ? [] : '']])));
}

function freshId(doc: PlanDoc, kind: string): string {
	const taken = new Set(doc.steps.map((s) => s.id));
	for (let n = 1; ; n++) {
		const id = n === 1 ? kind : `${kind}-${n}`;
		if (!taken.has(id)) return id;
	}
}

/** `step` as a `kind` step: the fields `kind` does not take go, and the ones it needs start blank. */
export function withKind(step: StepDoc, kind: Kind, doc: PlanDoc): StepDoc {
	const kept = Object.fromEntries(Object.entries(step).filter(([k]) => !OPTIONAL.includes(k as keyof StepDoc) || KIND_FIELDS[kind].includes(k as keyof StepDoc)));
	const next = { ...blank(kind), ...kept, kind } as StepDoc;
	if (kind !== 'deliver' && next.ok === undefined) next.ok = doc.steps.find((s) => s.kind === 'deliver' && s.id !== step.id)?.id;
	return next;
}

/** `doc` with a new `kind` step after step `at`, wired into the path: the step before leads to it, and it leads on to where that one went. */
export function addAfter(doc: PlanDoc, at: number, kind: Kind): PlanDoc {
	const before = doc.steps[at];
	const id = freshId(doc, kind);
	const onward = before?.ok ?? doc.steps[at + 1]?.id ?? doc.steps.find((s) => s.kind === 'deliver')?.id;
	const step: StepDoc = { id, title: `New ${kind} step`, kind, ...blank(kind) };
	if (kind !== 'deliver' && onward !== undefined) step.ok = onward;
	const steps = doc.steps.map((s, i) => (i === at && s.ok !== undefined ? { ...s, ok: id } : s));
	steps.splice(at + 1, 0, step);
	return { ...doc, steps };
}

/** `doc` without step `at`. An ok edge to it goes where it went; a fail edge to it goes. */
export function remove(doc: PlanDoc, at: number): PlanDoc {
	const gone = doc.steps[at];
	const steps = doc.steps.flatMap((s, i) => {
		if (i === at) return [];
		const next = { ...s };
		if (next.ok === gone.id) next.ok = gone.ok !== gone.id ? gone.ok : undefined;
		if (next.fail === gone.id) delete next.fail;
		if (next.ok === undefined) delete next.ok;
		return [next];
	});
	return { ...doc, steps };
}

/** `doc` with step `at` moved by `by` places. The edges stay, since they name ids. */
export function move(doc: PlanDoc, at: number, by: number): PlanDoc {
	const to = at + by;
	if (to < 0 || to >= doc.steps.length) return doc;
	const steps = [...doc.steps];
	const [step] = steps.splice(at, 1);
	steps.splice(to, 0, step);
	return { ...doc, steps };
}

/** What a step asks, short: its instructions, its command or its question. */
export function gist(step: StepDoc): string {
	const said = step.kind === 'check' ? step.command : step.kind === 'approve' ? step.question : step.instructions;
	const line = (said ?? '').replace(/\s+/g, ' ').trim();
	return line.length > 140 ? `${line.slice(0, 139)}…` : line;
}

/** Where a step leads, in words, such as "then 3. Live regression · on fail 5. Report". */
export function leads(doc: PlanDoc, step: StepDoc): string {
	if (step.kind === 'deliver') return 'ends the run';
	const name = (id: string | undefined) => {
		const i = doc.steps.findIndex((s) => s.id === id);
		return i < 0 ? (id ? `missing step ${id}` : 'nowhere yet') : `${i + 1}. ${doc.steps[i].title}`;
	};
	const approve = step.kind === 'approve';
	const ok = `${approve ? 'if yes' : 'then'} ${name(step.ok)}`;
	const fail = step.fail !== undefined ? name(step.fail) : approve ? 'the run ends declined' : 'the run fails';
	return `${ok} · ${approve ? 'if no' : 'on fail'} ${fail}`;
}

/** The plan text with `made_from` left out, so the controller marks it made from the prompt it holds now. */
export function reviewed(text: string): string {
	const read = readPlan(text);
	return 'doc' in read ? planText({ ...read.doc, made_from: undefined }) : text;
}
